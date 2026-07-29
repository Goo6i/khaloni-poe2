//! Windows injection stub — the real key-injection backend (SendInput)
//! lands in SP3. Public API mirrors platform/linux/inject.rs.

pub struct Injector;

impl Injector {
    pub fn new() -> anyhow::Result<Injector> {
        anyhow::bail!("windows backend lands in SP3")
    }

    /// Queues a copy of the hovered item; no-op until SP3 (constructing an
    /// `Injector` already bails, so this is unreachable in practice).
    pub fn submit(
        &self,
        _reply: std::sync::mpsc::Sender<anyhow::Result<String>>,
        _pre_delay_ms: u64,
    ) {
    }

    /// Queues a chat macro; no-op until SP3.
    pub fn type_text(&self, _msg: String, _open_delay_ms: u64) {}
}
