//! Typed shared-client commands on the existing native crypto-owner thread.
//! The original authenticated allocation survives queueing, metadata and output.
use super::*;
use mnema_private_native_crypto_owner::{
    ChatEventClaim, ChatOperation, NativeChatOwner, ReactionAction,
};
use serde::Deserialize;

#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Input {
    Create {
        body: String,
    },
    Reply {
        body: String,
        parent_receipt_id: Option<String>,
        quoted_receipt_id: Option<String>,
    },
    Edit {
        target_receipt_id: String,
        expected_revision: String,
        body: String,
    },
    Delete {
        target_receipt_id: String,
        expected_revision: String,
    },
    Reaction {
        target_receipt_id: String,
        emoji: String,
        action: Toggle,
    },
}
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Toggle {
    Toggle,
}

const MAX_CLAIMS: usize = 256;
/// Admitted mutations by client event id. An entry stays after its publish was
/// acknowledged so a repeated event id replays the exact operation, never its
/// inverse. Only acknowledged entries are evicted (oldest first) to make room;
/// unacknowledged ones are kept for their retry and bound the table.
#[derive(Default)]
pub(super) struct Claims {
    entries: std::collections::HashMap<Uuid, (Input, ChatOperation, bool)>,
    order: std::collections::VecDeque<Uuid>,
}

impl Claims {
    fn get(&self, event: &Uuid) -> Option<(&Input, &ChatOperation)> {
        self.entries.get(event).map(|(input, op, _)| (input, op))
    }

    fn insert(&mut self, event: Uuid, input: Input, operation: ChatOperation) -> Result<(), Error> {
        if self.entries.contains_key(&event) {
            return Ok(());
        }
        if self.entries.len() >= MAX_CLAIMS {
            let evictable = self
                .order
                .iter()
                .position(|id| self.entries.get(id).is_some_and(|entry| entry.2))
                .ok_or(Error::Busy)?;
            if let Some(id) = self.order.remove(evictable) {
                self.entries.remove(&id);
            }
        }
        self.entries.insert(event, (input, operation, false));
        self.order.push_back(event);
        Ok(())
    }

    fn is_full(&self) -> bool {
        self.entries.len() >= MAX_CLAIMS && !self.entries.values().any(|entry| entry.2)
    }

    fn acknowledge(&mut self, event: &Uuid) {
        if let Some(entry) = self.entries.get_mut(event) {
            entry.2 = true;
        }
    }
}

impl Input {
    fn validate(&self) -> Result<(), Error> {
        let body = match self {
            Self::Create { body } | Self::Reply { body, .. } | Self::Edit { body, .. } => {
                Some(body)
            }
            _ => None,
        };
        if body.is_some_and(|body| body.trim().is_empty() || body.encode_utf16().count() > 4000) {
            return Err(Error::InvalidInput);
        }
        if let Self::Reaction { emoji, .. } = self
            && (emoji.is_empty() || emoji.len() > 128)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

fn claim(
    chat: &mut NativeChatOwner,
    scope: Arc<NativeAuthenticatedScope>,
    event: Uuid,
    input: Input,
) -> Result<ChatEventClaim, Error> {
    let receipt = |id: String| canonical(&id).map(|id| id.to_string());
    let operation = match input {
        Input::Create { body } => ChatOperation::Create {
            message_id: event.to_string(),
            body,
            parent_id: None,
        },
        Input::Reply {
            body,
            parent_receipt_id,
            quoted_receipt_id,
        } => {
            let parent_id = parent_receipt_id.map(receipt).transpose()?;
            match quoted_receipt_id {
                Some(id) => ChatOperation::Reply {
                    message_id: event.to_string(),
                    body,
                    parent_id,
                    reply_to_id: receipt(id)?,
                },
                None if parent_id.is_some() => ChatOperation::Create {
                    message_id: event.to_string(),
                    body,
                    parent_id,
                },
                None => return Err(Error::InvalidInput),
            }
        }
        Input::Edit {
            target_receipt_id,
            expected_revision,
            body,
        } => ChatOperation::Edit {
            message_id: receipt(target_receipt_id)?,
            expected_revision: receipt(expected_revision)?,
            body,
        },
        Input::Delete {
            target_receipt_id,
            expected_revision,
        } => ChatOperation::Delete {
            message_id: receipt(target_receipt_id)?,
            expected_revision: receipt(expected_revision)?,
        },
        Input::Reaction {
            target_receipt_id,
            emoji,
            action: Toggle::Toggle,
        } => {
            let target = canonical(&target_receipt_id)?;
            let removed = chat
                .publish_cached_chat_event_message_retained(scope.clone(), target, |row| {
                    if row.deleted {
                        return Err(mnema_private_native_crypto_owner::Error::Invalid);
                    }
                    Ok(row
                        .reactions
                        .get(&emoji)
                        .is_some_and(|users| users.contains(&scope.account_id())))
                })
                .map_err(crypto_error)?
                .ok_or(Error::Denied)?;
            ChatOperation::Reaction {
                message_id: target.to_string(),
                emoji,
                action: if removed {
                    ReactionAction::Remove
                } else {
                    ReactionAction::Add
                },
            }
        }
    };
    ChatEventClaim::claim(operation).map_err(|_| Error::InvalidInput)
}

const WINDOW_ROWS: usize = 1024;
const WINDOW_BYTES: usize = 512 * 1024;

/// Newest-first window of whole pages holding at most `budget` rows. Older
/// pages are left out instead of failing, so a busy channel stays readable.
fn history(
    chat: &mut NativeChatOwner,
    scope: Arc<NativeAuthenticatedScope>,
    parent: Option<Uuid>,
    budget: usize,
) -> Result<Vec<serde_json::Value>, Error> {
    let mut rows = Vec::new();
    let mut before = None;
    loop {
        let (mut page, older, next) = chat
            .publish_cached_chat_event_history_page_retained(
                scope.clone(),
                parent,
                before,
                25,
                |page| {
                    let rows = page
                        .messages
                        .iter()
                        .map(serde_json::to_value)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| mnema_private_native_crypto_owner::Error::Invalid)?;
                    Ok((rows, page.has_older, page.next_before))
                },
            )
            .map_err(crypto_error)?;
        if rows.len() + page.len() > budget {
            return Ok(rows);
        }
        rows.append(&mut page);
        if !older {
            return Ok(rows);
        }
        let next = next.ok_or(Error::Protocol)?;
        if before.is_some_and(|old| next >= old) {
            return Err(Error::Protocol);
        }
        before = Some(next);
    }
}

/// Author object of one row. A member the instance no longer lists (deactivated
/// account, disabled bot) keeps its row with an empty profile instead of
/// failing the whole snapshot; the server omits `avatar_url` when none is set.
fn author_fields(
    member: Option<&serde_json::Value>,
    account: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let text = |field: &str| {
        member
            .and_then(|member| member.get(field))
            .filter(|value| value.is_string())
            .cloned()
            .unwrap_or_else(|| serde_json::json!(""))
    };
    let mut fields = serde_json::Map::new();
    fields.insert("id".into(), serde_json::json!(account));
    for field in ["username", "display_name", "avatar_url"] {
        fields.insert(field.into(), text(field));
    }
    fields
}

/// Drop rows from the end (oldest pages, then thread replies of the oldest
/// roots) until the serialized snapshot fits `limit` bytes.
fn trim_to_bytes(
    rows: &mut Vec<serde_json::Value>,
    channel: Uuid,
    limit: usize,
) -> Result<(), Error> {
    let overhead = serde_json::to_vec(&serde_json::json!({"channel_id":channel,"messages":[]}))
        .map_err(|_| Error::Protocol)?
        .len();
    let sizes = rows
        .iter()
        .map(|row| serde_json::to_vec(row).map(|bytes| bytes.len() + 1))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::Protocol)?;
    let mut total = overhead + sizes.iter().sum::<usize>();
    let mut keep = rows.len();
    while total > limit && keep > 0 {
        keep -= 1;
        total -= sizes[keep];
    }
    if keep == 0 && !rows.is_empty() {
        return Err(Error::BodyLimit);
    }
    rows.truncate(keep);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process(
    chat: &mut NativeChatOwner,
    claims: &mut Claims,
    app: &tauri::AppHandle,
    client: &NativeClient,
    lease: &NativeWindowLease,
    control: &Arc<Control>,
    scope: Arc<NativeAuthenticatedScope>,
    event: Option<Uuid>,
    input: Option<Input>,
    output: Channel<serde_json::Value>,
) -> Result<Option<Uuid>, Error> {
    client.check_authenticated_scope(lease, &scope)?;
    let affected = match (event, input) {
        (Some(event), Some(input)) => {
            let original = input.clone();
            let claim = if let Some((previous, operation)) = claims.get(&event) {
                if previous != &input {
                    return Err(Error::Denied);
                }
                // An unknown ACK may already have toggled the native history.
                // Retry this exact admitted operation, never its inverse.
                ChatEventClaim::claim(operation.clone()).map_err(|_| Error::InvalidInput)?
            } else {
                if claims.is_full() {
                    return Err(Error::Busy);
                }
                claim(chat, scope.clone(), event, input)?
            };
            let prepared = chat
                .prepare_chat_event_retained(scope.clone(), event, &claim)
                .map_err(crypto_error)?;
            claims.insert(event, original, claim.operation().clone())?;
            let message = chat
                .publish_chat_event(prepared, |publication| {
                    let matching = publication
                        .receipts
                        .iter()
                        .filter(|row| {
                            row.client_event_id == event && row.account_id == scope.account_id()
                        })
                        .collect::<Vec<_>>();
                    if matching.len() != 1 {
                        return Err(mnema_private_native_crypto_owner::Error::Binding);
                    }
                    Ok(matching[0].message_id)
                })
                .await
                .map_err(crypto_error)?;
            claims.acknowledge(&event);
            Some(message)
        }
        (None, None) => {
            // Bounded catch-up of actual event pages. No mutation is retried.
            let mut received = 0;
            for _ in 0..=1024 {
                let count = chat
                    .receive_chat_event_page_retained(scope.clone(), |publication| {
                        Ok(publication.receipts.len())
                    })
                    .await
                    .map_err(crypto_error)?;
                if count == 0 {
                    break;
                }
                received += count;
                if received > 1024 {
                    return Err(Error::BodyLimit);
                }
            }
            None
        }
        _ => return Err(Error::InvalidInput),
    };
    let mut rows = history(chat, scope.clone(), None, WINDOW_ROWS)?;
    let roots = rows
        .iter()
        .map(|row| {
            row.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::Protocol)
                .and_then(canonical)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for root in roots {
        let budget = WINDOW_ROWS.saturating_sub(rows.len());
        if budget == 0 {
            break;
        }
        rows.extend(history(chat, scope.clone(), Some(root), budget)?);
    }
    // Author display data comes from this instance's real members response.
    // No usernames, dates or revisions are made up.
    let publication = client
        .request_retained_authenticated(
            lease,
            scope.clone(),
            Uuid::new_v4(),
            NativeRequest::Metadata {
                resource: MetadataResource::Members,
            },
        )
        .await?;
    let members = client.commit(lease, publication)?;
    if members.status != 200 {
        return Err(Error::Protocol);
    }
    let members = members.body.as_array().ok_or(Error::Protocol)?;
    let members = members
        .iter()
        .filter_map(|member| {
            let id = member.get("id").and_then(serde_json::Value::as_str)?;
            Some((id, member))
        })
        .collect::<std::collections::HashMap<_, _>>();
    for row in &mut rows {
        let account = row
            .get("account_id")
            .and_then(serde_json::Value::as_str)
            .ok_or(Error::Protocol)?;
        let author = author_fields(members.get(account).copied(), account);
        let row = row.as_object_mut().ok_or(Error::Protocol)?;
        row.insert("channel_id".into(), serde_json::json!(control.channel));
        row.insert("author".into(), serde_json::Value::Object(author));
        // This native version admits text only; attachments/pins/mentions have
        // no events in its closed schema. These values assert no extra grants.
        row.insert("attachments".into(), serde_json::json!([]));
        row.insert("is_pinned".into(), serde_json::json!(false));
        row.insert("mentions".into(), serde_json::json!([]));
    }
    trim_to_bytes(&mut rows, control.channel, WINDOW_BYTES)?;
    let snapshot = serde_json::json!({"channel_id":control.channel,"messages":rows});
    client.with_authenticated_publication(lease, &scope, || {
        let state = app.state::<RuntimeState>();
        let registry = state.registry.try_lock().map_err(|_| Error::Stale)?;
        let entry = registry.as_ref().ok_or(Error::Stale)?;
        if control.stop.load(Ordering::Acquire)
            || entry.document != NativeDocument::App
            || entry.lease.context_nonce() != lease.context_nonce()
        {
            return Err(Error::Stale);
        }
        output.send(snapshot).map_err(|_| Error::Stale)
    })??;
    Ok(affected)
}

#[allow(clippy::too_many_arguments)]
async fn command(
    window: WebviewWindow,
    state: tauri::State<'_, RuntimeState>,
    context: String,
    profile_intent: String,
    authentication_intent: String,
    channel_id: String,
    event: Option<String>,
    input: Option<Input>,
    output: Channel<serde_json::Value>,
) -> NativeReply {
    let result = async {
        let lease = require(&window, &state, Some(&context))?;
        let profile = canonical(&profile_intent)?;
        state.client.check_profile_intent(&lease, profile)?;
        let channel = canonical(&channel_id)?;
        let event = event.map(|id| canonical(&id)).transpose()?;
        if let Some(input) = &input {
            input.validate()?;
        }
        let control = state.core.current_profile(&lease, None, profile)?;
        let scope = Arc::new(
            state
                .client
                .authenticated_scope_with_intent(&lease, canonical(&authentication_intent)?)?,
        );
        if control.channel != channel || !control.same_family(&scope) {
            return Err(Error::Denied);
        }
        if control
            .status
            .lock()
            .map_err(|_| Error::Internal)?
            .as_ref()
            .is_none_or(|status| status.state != "root_saved")
        {
            return Err(Error::QualificationRequired);
        }
        let (reply, answer) = oneshot::channel();
        state
            .client
            .with_authenticated_publication(&lease, &scope, || {
                control
                    .jobs
                    .try_send(Job::TypedChat {
                        scope: scope.clone(),
                        event,
                        input,
                        output,
                        reply,
                    })
                    .map_err(|_| Error::Busy)
            })??;
        let affected = answer.await.map_err(|_| Error::Stale)??;
        require(&window, &state, Some(&context))?;
        state.client.check_authenticated_scope(&lease, &scope)?;
        if control.stop.load(Ordering::Acquire) {
            return Err(Error::Stale);
        }
        Ok::<_, Error>(match affected {
            Some(id) => serde_json::json!({"state":"completed","message_id":id}),
            None => serde_json::json!({"state":"completed"}),
        })
    }
    .await;
    match result {
        Ok(body) => reply(context, body),
        Err(error) => rejected(context, error),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn native_chat_snapshot(
    window: WebviewWindow,
    state: tauri::State<'_, RuntimeState>,
    context: String,
    profile_intent: String,
    authentication_intent: String,
    channel_id: String,
    on_messages: Channel<serde_json::Value>,
) -> Result<NativeReply, ()> {
    Ok(command(
        window,
        state,
        context,
        profile_intent,
        authentication_intent,
        channel_id,
        None,
        None,
        on_messages,
    )
    .await)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn native_chat_mutate(
    window: WebviewWindow,
    state: tauri::State<'_, RuntimeState>,
    context: String,
    profile_intent: String,
    authentication_intent: String,
    channel_id: String,
    client_event_id: String,
    input: Input,
    on_messages: Channel<serde_json::Value>,
) -> Result<NativeReply, ()> {
    Ok(command(
        window,
        state,
        context,
        profile_intent,
        authentication_intent,
        channel_id,
        Some(client_event_id),
        Some(input),
        on_messages,
    )
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op() -> ChatOperation {
        ChatOperation::Create {
            message_id: Uuid::new_v4().to_string(),
            body: "x".into(),
            parent_id: None,
        }
    }
    fn input() -> Input {
        Input::Create { body: "x".into() }
    }

    #[test]
    fn acknowledged_claims_are_evicted_so_mutations_keep_working() {
        let mut claims = Claims::default();
        for _ in 0..(MAX_CLAIMS * 3) {
            let event = Uuid::new_v4();
            assert!(!claims.is_full());
            claims.insert(event, input(), op()).unwrap();
            claims.acknowledge(&event);
        }
        assert!(claims.entries.len() <= MAX_CLAIMS);
    }

    #[test]
    fn unacknowledged_claims_stay_for_their_retry_and_bound_the_table() {
        let mut claims = Claims::default();
        let first = Uuid::new_v4();
        claims.insert(first, input(), op()).unwrap();
        for _ in 1..MAX_CLAIMS {
            claims.insert(Uuid::new_v4(), input(), op()).unwrap();
        }
        assert!(claims.is_full());
        assert!(matches!(
            claims.insert(Uuid::new_v4(), input(), op()),
            Err(Error::Busy)
        ));
        assert!(claims.get(&first).is_some());
        claims.acknowledge(&first);
        claims.insert(Uuid::new_v4(), input(), op()).unwrap();
        assert!(claims.get(&first).is_none());
    }

    #[test]
    fn author_without_avatar_or_member_still_renders() {
        let member = serde_json::json!({"id":"a","username":"u","display_name":"U"});
        let fields = author_fields(Some(&member), "a");
        assert_eq!(fields["username"], "u");
        assert_eq!(fields["avatar_url"], "");
        let gone = author_fields(None, "b");
        assert_eq!(gone["id"], "b");
        assert_eq!(gone["username"], "");
    }

    #[test]
    fn oversized_snapshot_keeps_a_newest_window_instead_of_failing() {
        let channel = Uuid::new_v4();
        let mut rows = (0..10)
            .map(|n| serde_json::json!({"n": n, "body": "a".repeat(100)}))
            .collect::<Vec<_>>();
        trim_to_bytes(&mut rows, channel, 450).unwrap();
        assert!(!rows.is_empty() && rows.len() < 10);
        assert_eq!(rows[0]["n"], 0);
        let size = serde_json::to_vec(&serde_json::json!({"channel_id":channel,"messages":rows}))
            .unwrap()
            .len();
        assert!(size <= 450);
        let mut single = vec![serde_json::json!({"body": "a".repeat(1000)})];
        assert!(trim_to_bytes(&mut single, channel, 450).is_err());
    }
    #[test]
    fn renderer_cannot_supply_authority_or_unknown_operation_fields() {
        assert!(
            serde_json::from_value::<Input>(serde_json::json!({"kind":"create","body":"hello"}))
                .is_ok()
        );
        for extra in [
            "account_id",
            "device_id",
            "created_at",
            "admin",
            "channel_id",
            "grant",
        ] {
            let mut value = serde_json::json!({"kind":"create","body":"hello"});
            value[extra] = serde_json::json!("untrusted");
            assert!(serde_json::from_value::<Input>(value).is_err());
        }
        assert!(serde_json::from_value::<Input>(serde_json::json!({"kind":"reaction","target_receipt_id":Uuid::new_v4(),"emoji":"a","action":"add"})).is_err());
    }

    #[test]
    fn typed_body_budget_matches_shared_utf16_decoder_before_queueing() {
        assert!(
            Input::Create {
                body: "a".repeat(4000)
            }
            .validate()
            .is_ok()
        );
        assert!(
            Input::Create {
                body: "a".repeat(4001)
            }
            .validate()
            .is_err()
        );
        assert!(
            Input::Create {
                body: "🔒".repeat(2000)
            }
            .validate()
            .is_ok()
        );
        assert!(
            Input::Create {
                body: "🔒".repeat(2001)
            }
            .validate()
            .is_err()
        );
        assert!(
            Input::Create {
                body: " \t\n".into()
            }
            .validate()
            .is_err()
        );
    }
}
