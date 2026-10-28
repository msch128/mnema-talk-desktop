//! Auth + fixed EMPTY ciphertext-page proof only. No synthetic ciphertext is
//! inserted into the real backend; actual protected publishing needs Core proof.
#[path = "support/transport_fixture.rs"]
mod transport_fixture;
use mnema_private_native_client_broker::{NativeOpaqueRelayOperation, NativeRequest, Password};
use uuid::Uuid;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Refuses to run without the explicit fixture flag; origin and login come
    // from the one validated bootstrap read, never from a second unchecked read.
    let transport_fixture::Qualified {
        client,
        origin,
        username,
        password,
        binding,
    } = transport_fixture::qualified()?;
    let channel = Uuid::parse_str(binding["channel_id"].as_str().ok_or("missing channel")?)?;
    let window = client.attach_main_window()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async{
        let connected=client.request(&window,NativeRequest::Connect{address:origin}).await?;client.commit(&window,connected)?;
        let login=client.request(&window,NativeRequest::Login{username:username.to_owned(),password:Password::from_native_input(password.to_owned())?}).await?;client.commit(&window,login)?;
        let result=client.request_opaque_relay(&window,client.authenticated_scope(&window)?,NativeOpaqueRelayOperation::Page{channel,after:0}).await?;
        let receipt=result.opaque_relay_receipt().ok_or("missing native receipt")?;
        if !receipt.records().is_empty() || receipt.next_after()!=0 || receipt.is_publish_ack(){return Err("owned fixture is not empty".into())}
        client.with_opaque_relay_publication(&window,result,|_|())?;
        let logout=client.request(&window,NativeRequest::Logout).await?;client.commit(&window,logout)?;
        println!("PASS: actual PG18 CA-verified HTTPS native login/fixed limit10 empty opaque page/current-scope final commit/logout; no ciphertext insertion");
        Ok::<(),Box<dyn std::error::Error>>(())
    })
}
