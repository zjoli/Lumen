use super::*;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub message: AgentMessage,
}
pub struct SessionStore {
    path: PathBuf,
}
impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub async fn append(&self, message: &AgentMessage) -> io::Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        f.write_all(
            serde_json::to_string(&SessionRecord {
                message: message.clone(),
            })
            .unwrap()
            .as_bytes(),
        )
        .await?;
        f.write_all(b"\n").await
    }
    pub async fn load(&self) -> io::Result<Vec<AgentMessage>> {
        if !Path::new(&self.path).exists() {
            return Ok(vec![]);
        }
        let f = File::open(&self.path).await?;
        let mut r = BufReader::new(f).lines();
        let mut out = vec![];
        while let Some(line) = r.next_line().await? {
            if let Ok(x) = serde_json::from_str::<SessionRecord>(&line) {
                out.push(x.message)
            }
        }
        Ok(out)
    }
}
