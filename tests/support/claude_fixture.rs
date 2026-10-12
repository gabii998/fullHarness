// Native offline protocol fixture. Does not access an account or external network.
use std::{io::{Read, Write}, net::TcpStream};
fn event(text: &str) { println!("{text}"); std::io::stdout().flush().unwrap(); }
fn rpc(host: &str, body: &str) -> String {
    let mut socket = TcpStream::connect(host).unwrap();
    let token = std::env::var("ASCURRA_MCP_TOKEN").unwrap();
    write!(socket, "POST /mcp HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut output = String::new(); socket.read_to_string(&mut output).unwrap(); output
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") { println!("2.1.295 (Claude Code)"); return; }
    if args.iter().any(|a| a == "auth") { println!("{{\"loggedIn\":true,\"authMethod\":\"claude.ai\"}}"); return; }
    let arg = |name: &str| args[args.iter().position(|a| a == name).unwrap()+1].as_str();
    if args.iter().any(|a| a == "--no-session-persistence") {
        use std::io::BufRead;
        assert_eq!(arg("--tools"), "");
        assert_eq!(arg("--mcp-config"), r#"{"mcpServers":{}}"#);
        let mut input = std::io::stdin().lock();
        let mut line = String::new(); input.read_line(&mut line).unwrap();
        assert!(line.contains("initialize") && !line.contains(r#""type":"user""#));
        event(r#"{"type":"control_response","response":{"subtype":"success","request_id":"quota-init","response":{}}}"#);
        line.clear(); input.read_line(&mut line).unwrap(); assert!(line.contains("get_usage"));
        event(r#"{"type":"control_response","response":{"subtype":"success","request_id":"quota-read","response":{"subscription_type":"pro","rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":25},"seven_day":{"utilization":80}}}}}"#);
        std::thread::sleep(std::time::Duration::from_secs(600));
        return;
    }
    assert_eq!(arg("--tools"), "");
    assert!(arg("--system-prompt").contains("Project fixture instructions"));
    assert!(args.iter().any(|a| a == "--restricted"));
    assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    assert!(std::env::var_os("ANTHROPIC_API_KEY").is_none());
    let session = if args.iter().any(|a| a == "--resume") { arg("--resume") } else { arg("--session-id") };
    let mut input = String::new(); std::io::stdin().read_to_string(&mut input).unwrap();
    assert!(input.contains(session));
    if input.contains("write") {
        assert!(input.contains(r#""type":"image""#));
        assert!(input.contains(r#""media_type":"image/png""#));
        assert!(input.contains(r#""type":"base64""#));
        assert!(input.contains("iVBORw0KGgo"));
        assert!(!input.contains("harness-image:"));
    }
    let config = arg("--mcp-config");
    let host = config.split("http://").nth(1).unwrap().split('/').next().unwrap();
    assert!(rpc(host, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).contains("protocolVersion"));
    assert!(rpc(host, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).contains("read_file"));
    event(&format!(r#"{{"type":"system","subtype":"init","session_id":"{session}","model":"claude-fixture","tools":["mcp__harness__read_file","mcp__harness__write_file"],"mcp_servers":[{{"name":"harness","status":"connected"}}]}}"#));
    if input.contains("cancel") { std::thread::sleep(std::time::Duration::from_secs(600)); return; }
    let write = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_file","arguments":{"path":"fixture.txt","content":"approved"}}}"#;
    let response = rpc(host, write);
    assert!(response.contains("200 OK"));
    assert!(rpc(host, write).contains("Duplicate"));
    let denied = input.contains("deny");
    assert_eq!(std::path::Path::new("fixture.txt").exists(), !denied);
    if !denied { assert!(rpc(host, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"fixture.txt","offset":0,"limit":100}}}"#).contains("approved")); }
    event(r#"{"type":"stream_event","event":{"type":"message_start"}}"#);
    event(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Parcial"}}}"#);
    let answer = if args.iter().any(|a| a == "--resume") { "Reanudado" } else { "Respuesta final" };
    event(&format!(r#"{{"type":"assistant","message":{{"id":"msg-{session}","model":"claude-fixture","content":[{{"type":"text","text":"{answer}"}}],"usage":{{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":4}}}}}}"#));
    event(&format!(r#"{{"type":"result","subtype":"success","is_error":false,"uuid":"turn-{session}-{}","result":"{answer}","usage":{{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":4}},"modelUsage":{{"claude-fixture":{{"contextWindow":200000}}}}}}"#, if args.iter().any(|a| a == "--resume") { 2 } else { 1 }));
}
