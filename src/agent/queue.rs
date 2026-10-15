use super::*;
use tokio::sync::mpsc;
pub enum AgentInput {
    User(Vec<ContentPart>),
    Steering(Vec<ContentPart>),
    FollowUp(Vec<ContentPart>),
    Stop,
}
pub struct MessageQueue {
    tx: mpsc::Sender<AgentInput>,
    rx: mpsc::Receiver<AgentInput>,
}
impl MessageQueue {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self { tx, rx }
    }
    pub fn sender(&self) -> mpsc::Sender<AgentInput> {
        self.tx.clone()
    }
    pub async fn send(&self, x: AgentInput) -> Result<(), mpsc::error::SendError<AgentInput>> {
        self.tx.send(x).await
    }
    pub async fn recv(&mut self) -> Option<AgentInput> {
        self.rx.recv().await
    }
}
