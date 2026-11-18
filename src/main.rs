mod adb;
mod comparison;
mod elements;
mod mcp;
mod perf;

use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::mcp::McpServer;

#[tokio::main]
async fn main() {
    let server = McpServer::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_message(trimmed).await {
            if let Ok(mut encoded) = serde_json::to_string(&resp) {
                encoded.push('\n');
                if stdout.write_all(encoded.as_bytes()).await.is_err() {
                    break;
                }
                if stdout.flush().await.is_err() {
                    break;
                }
            }
        }
    }
}
