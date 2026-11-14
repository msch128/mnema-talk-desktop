//! "Update now" and a running Talk: the decision, the question and the wait.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn update_now_asks_first_only_during_a_talk_and_ends_a_wait_when_pressed_again() {
    use NowChoice::*;
    for phase in [Phase::Available, Phase::Ready, Phase::Failed] {
        assert_eq!(now_choice(phase, true, false, false), Install, "{phase:?}");
        assert_eq!(
            now_choice(phase, true, true, false),
            AskAboutTalk,
            "{phase:?}"
        );
        assert_eq!(now_choice(phase, true, true, true), Install, "{phase:?}");
        // A failed check offers no version: there is nothing to install yet.
        assert_eq!(now_choice(phase, false, false, false), Show, "{phase:?}");
    }
    assert_eq!(now_choice(Phase::Waiting, true, true, false), StopWaiting);
    for phase in [
        Phase::Idle,
        Phase::Checking,
        Phase::UpToDate,
        Phase::Downloading,
        Phase::Installing,
    ] {
        assert_eq!(now_choice(phase, true, false, false), Show, "{phase:?}");
    }
}

#[test]
fn the_wait_ends_after_a_few_quiet_polls_or_at_once_when_forced() {
    assert!(!call_over(0, false));
    assert!(!call_over(QUIET_POLLS - 1, false));
    assert!(call_over(QUIET_POLLS, false));
    assert!(call_over(0, true));
    // One missed report is not the end of a Talk.
    const { assert!(QUIET_POLLS >= 2) };
}

#[test]
fn a_page_that_stopped_reporting_its_talk_is_not_in_one() {
    let mut g = crate::web_gaming::Gaming::default();
    assert!(!g.talking(std::time::Duration::from_secs(5)));
    g.test_talk(true);
    assert!(g.talking(std::time::Duration::from_secs(5)));
    assert!(!g.talking(std::time::Duration::ZERO), "a stale report");
    g.test_talk(false);
    assert!(!g.talking(std::time::Duration::from_secs(5)));
}

struct Running {
    _app: tauri::App<MockRuntime>,
    handle: tauri::AppHandle<MockRuntime>,
    talking: Arc<AtomicBool>,
    restarted: Arc<AtomicBool>,
    folder: PathBuf,
}

fn portable_in_a_talk(name: &str) -> Running {
    let keys = keys();
    let dir = data_dir(name);
    let folder = dir.join("app");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(PORTABLE_EXE), b"old program").unwrap();
    let mut rel = release("0.2.0", &keys);
    rel.portable = Some(portable_zip(b"new program"));
    let port = publish(&rel);
    let app = app_built(&keys, port, &dir, false, true);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();
    let talking = Arc::new(AtomicBool::new(true));
    let probe = talking.clone();
    state.set_talk_probe(move || probe.load(Ordering::SeqCst));
    let restarted = Arc::new(AtomicBool::new(false));
    let flag = restarted.clone();
    *state.restart.lock().unwrap() = Some(Arc::new(move || flag.store(true, Ordering::SeqCst)));
    assert!(run(look(&handle, false)));
    Running {
        _app: app,
        handle,
        talking,
        restarted,
        folder,
    }
}

fn wait_for(what: impl Fn() -> bool) -> bool {
    for _ in 0..500 {
        if what() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn after_the_call_the_verified_update_waits_and_installs_once_the_talk_has_ended() {
    let r = portable_in_a_talk("after-call");
    let state = r.handle.state::<NativeUpdates>();
    assert_eq!(request_now(&r.handle, false), NowChoice::AskAboutTalk);
    assert!(state.snapshot().confirm_call, "the window asks");
    assert_eq!(
        state.snapshot().phase,
        Phase::Available,
        "nothing started yet"
    );

    let handle = r.handle.clone();
    tauri::async_runtime::spawn(async move { install(&handle, Mode::AfterCall).await });
    assert!(
        wait_for(|| state.snapshot().phase == Phase::Waiting),
        "{:?}",
        state.snapshot()
    );
    let snap = state.snapshot();
    assert_eq!(
        (snap.mode, snap.confirm_call),
        (Some(Mode::AfterCall), false)
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(state.snapshot().phase, Phase::Waiting, "the Talk goes on");
    assert!(!r.restarted.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read(r.folder.join(PORTABLE_EXE)).unwrap(),
        b"old program"
    );

    r.talking.store(false, Ordering::SeqCst);
    assert!(
        wait_for(|| r.restarted.load(Ordering::SeqCst)),
        "{:?}",
        state.snapshot()
    );
    assert_eq!(
        std::fs::read(r.folder.join(PORTABLE_EXE)).unwrap(),
        b"new program"
    );
}

#[test]
fn update_now_while_waiting_installs_without_waiting_for_the_talk() {
    let r = portable_in_a_talk("after-call-forced");
    let state = r.handle.state::<NativeUpdates>();
    let handle = r.handle.clone();
    tauri::async_runtime::spawn(async move { install(&handle, Mode::AfterCall).await });
    assert!(wait_for(|| state.snapshot().phase == Phase::Waiting));
    assert_eq!(request_now(&r.handle, false), NowChoice::StopWaiting);
    assert!(
        wait_for(|| r.restarted.load(Ordering::SeqCst)),
        "{:?}",
        state.snapshot()
    );
    assert!(r.talking.load(Ordering::SeqCst), "still in the Talk");
}

#[test]
fn update_now_anyway_during_a_talk_installs_at_once() {
    let r = portable_in_a_talk("now-anyway");
    let state = r.handle.state::<NativeUpdates>();
    assert_eq!(request_now(&r.handle, true), NowChoice::Install);
    assert!(
        wait_for(|| r.restarted.load(Ordering::SeqCst)),
        "{:?}",
        state.snapshot()
    );
}
