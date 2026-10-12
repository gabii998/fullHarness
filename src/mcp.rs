//! Per-turn, authenticated loopback tool transport. No provider credentials pass here.
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc, oneshot},
    task::JoinSet,
};

pub struct Call {
    pub name: String,
    pub arguments: Value,
    pub reply: oneshot::Sender<String>,
}
pub struct Bridge {
    pub url: String,
    pub token: String,
    pub calls: mpsc::Receiver<Call>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Bridge {
    pub async fn new() -> Result<Self, String> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| "No se pudo iniciar el puente de herramientas.")?;
        let url = format!(
            "http://{}/mcp",
            listener.local_addr().map_err(|_| "Puerto MCP inválido.")?
        );
        let token = uuid::Uuid::new_v4().to_string();
        let auth = token.clone();
        let (sender, calls) = mpsc::channel(8);
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if handlers.len() < 8 => {
                        let Ok((socket, _)) = accepted else { break };
                        let (auth, sender, seen) = (auth.clone(), sender.clone(), seen.clone());
                        handlers.spawn(async move { let _ = serve(socket, &auth, sender, seen).await; });
                    }
                    _ = handlers.join_next(), if !handlers.is_empty() => {}
                }
            }
        });
        Ok(Self {
            url,
            token,
            calls,
            task,
        })
    }
}
async fn respond(socket: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await
}
async fn serve(
    mut socket: TcpStream,
    token: &str,
    sender: mpsc::Sender<Call>,
    seen: Arc<Mutex<HashSet<String>>>,
) -> std::io::Result<()> {
    let mut header = Vec::new();
    let header_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 16 * 1024 {
            return respond(&mut socket, "431 Request Header Fields Too Large", "").await;
        }
        let byte = tokio::time::timeout_at(header_deadline, socket.read_u8()).await??;
        header.push(byte);
    }
    let header = String::from_utf8_lossy(&header);
    let lines: Vec<_> = header.split("\r\n").collect();
    let field = |key: &str| {
        lines
            .iter()
            .filter_map(|line| line.split_once(':'))
            .filter(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.trim())
            .collect::<Vec<_>>()
    };
    if lines.first() != Some(&"POST /mcp HTTP/1.1") {
        return respond(&mut socket, "405 Method Not Allowed", "").await;
    }
    let lengths = field("Content-Length");
    let length = if lengths.len() == 1 {
        lengths[0].parse::<usize>().ok()
    } else {
        None
    };
    let Some(length) = length.filter(|n| *n <= 1024 * 1024) else {
        return respond(&mut socket, "413 Payload Too Large", "").await;
    };
    let mut body = vec![0; length];
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        socket.read_exact(&mut body),
    )
    .await??;
    if field("Authorization") != vec![format!("Bearer {token}").as_str()] {
        return respond(&mut socket, "401 Unauthorized", "").await;
    }
    if !field("Origin").is_empty() || !field("Transfer-Encoding").is_empty() {
        return respond(&mut socket, "403 Forbidden", "").await;
    }
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return respond(&mut socket, "400 Bad Request", "").await;
    };
    if request["jsonrpc"] != "2.0" {
        return respond(&mut socket, "400 Bad Request", "").await;
    }
    if request.get("id").is_none() {
        return respond(&mut socket, "202 Accepted", "").await;
    }
    let id = request["id"].clone();
    let result = match request["method"].as_str().unwrap_or("") {
        "initialize" => {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"Ascurra Harness","version":"0.1.0"}})
        }
        "ping" => json!({}),
        "tools/list" => {
            json!({"tools":crate::tool_runtime::schemas()[0]["tools"].as_array().unwrap().iter().map(|tool| json!({"name":tool["name"],"description":tool["description"],"inputSchema":tool["parameters"]})).collect::<Vec<_>>()})
        }
        "tools/call" => {
            let mut seen = seen.lock().await;
            let key = id.to_string();
            if seen.len() >= 128 || !seen.insert(key) {
                drop(seen);
                return respond(&mut socket, "200 OK", &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Duplicate or excessive tool request; not executed"}}).to_string()).await;
            }
            drop(seen);
            let name = request["params"]["name"].as_str().unwrap_or("").to_string();
            if !crate::tool_runtime::schemas()[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == name)
            {
                return respond(&mut socket, "200 OK", &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Unknown tool"}}).to_string()).await;
            }
            let (reply, receive) = oneshot::channel();
            if sender
                .send(Call {
                    name,
                    arguments: request["params"]["arguments"].clone(),
                    reply,
                })
                .await
                .is_err()
            {
                return Ok(());
            }
            let Ok(text) = receive.await else {
                return Ok(());
            };
            json!({"content":[{"type":"text","text":text}]})
        }
        _ => return respond(
            &mut socket,
            "200 OK",
            &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                .to_string(),
        )
        .await,
    };
    respond(
        &mut socket,
        "200 OK",
        &json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn authorization_and_duplicate_ids_prevent_unintended_execution() {
        let mut bridge = Bridge::new().await.unwrap();
        let client = reqwest::Client::new();
        let list: Value = client
            .post(&bridge.url)
            .bearer_auth(&bridge.token)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            list["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "http_collections")
        );
        assert!(
            list["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "http_request")
        );
        let body = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"README.md","offset":0,"limit":100}}});
        let unauthorized = client.post(&bridge.url).json(&body).send().await.unwrap();
        assert_eq!(unauthorized.status(), 401);
        assert!(bridge.calls.try_recv().is_err());
        let request = client
            .post(&bridge.url)
            .bearer_auth(&bridge.token)
            .json(&body);
        let first =
            tokio::spawn(
                async move { request.send().await.unwrap().json::<Value>().await.unwrap() },
            );
        let call = tokio::time::timeout(std::time::Duration::from_secs(2), bridge.calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.name, "read_file");
        let repeated: Value = client
            .post(&bridge.url)
            .bearer_auth(&bridge.token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(repeated["error"]["code"], -32600);
        assert!(bridge.calls.try_recv().is_err());
        call.reply.send(r#"{"ok":true}"#.into()).unwrap();
        assert_eq!(
            first.await.unwrap()["result"]["content"][0]["text"],
            r#"{"ok":true}"#
        );
        let origin = client
            .post(&bridge.url)
            .bearer_auth(&bridge.token)
            .header("Origin", "https://untrusted.example")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(origin.status(), 403);
    }
}
