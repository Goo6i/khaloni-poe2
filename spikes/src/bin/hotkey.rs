use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session().await?;
    let shortcuts =
        vec![NewShortcut::new("spike-check", "khaloni-poe2 spike hotkey").preferred_trigger("F6")];
    gs.bind_shortcuts(&session, &shortcuts, None).await?.response()?;
    eprintln!("bound; press F6 anywhere (including inside the game). Ctrl+C here to quit.");

    let mut activated = gs.receive_activated().await?;
    while let Some(a) = activated.next().await {
        println!("activated: {:?}", a.shortcut_id());
    }
    Ok(())
}
