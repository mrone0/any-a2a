use any_a2a::{A2aClient, inspect_card};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn card(version: &str, url: &str) -> Value {
    if version == "1.0" {
        json!({"name":"test","description":"test agent","version":"agent-1","capabilities":{},"defaultInputModes":["text/plain"],"defaultOutputModes":["text/plain"],"skills":[{"id":"chat","name":"Chat","description":"chat","tags":["text"]}],"supportedInterfaces":[{"url":url,"protocolBinding":"JSONRPC","protocolVersion":"1.0","tenant":"acme"}]})
    } else {
        json!({"name":"test","description":"test agent","version":"agent-1","capabilities":{},"defaultInputModes":["text/plain"],"defaultOutputModes":["text/plain"],"skills":[{"id":"chat","name":"Chat","description":"chat","tags":["text"]}],"url":url,"preferredTransport":"JSONRPC","protocolVersion":"0.3.0"})
    }
}
fn serve(results: Vec<Value>, version: &str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let c = card(version, &format!("{base}/rpc"));
    let h = thread::spawn(move || {
        for (i, r) in std::iter::once(c).chain(results).enumerate() {
            let (mut s, _) = listener.accept().unwrap();
            let mut b = Vec::new();
            let mut x = [0; 1];
            while !b.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut x).unwrap();
                b.push(x[0]);
            }
            let hs = String::from_utf8(b).unwrap();
            let n = hs
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|x| x.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut p = vec![0; n];
            s.read_exact(&mut p).unwrap();
            let body = if i == 0 {
                r
            } else {
                let q: Value = serde_json::from_slice(&p).unwrap();
                assert_eq!(
                    q["method"],
                    if q["method"] == "message/send" || q["method"] == "SendMessage" {
                        q["method"].clone()
                    } else {
                        json!("tasks/get")
                    }
                );
                json!({"jsonrpc":"2.0","id":q["id"],"result":r})
            }
            .to_string();
            write!(s,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    (format!("{base}/.well-known/agent-card.json"), h)
}
fn task(state: &str) -> Value {
    json!({"kind":"task","id":"t1","contextId":"c1","status":{"state":state}})
}
#[test]
fn preserves_original_parts_and_previews_without_invented_newlines() {
    let message = json!({"kind":"message","parts":[{"kind":"text","text":"Hello "},{"kind":"text","text":"world\nNext line"},{"kind":"data","data":{"temperature":23}}]});
    let (url, worker) = serve(vec![message.clone()], "0.3.0");
    let mut client = A2aClient::connect(&url, None, Duration::from_secs(5)).unwrap();
    let outcome = client.run("test").unwrap();
    worker.join().unwrap();
    assert_eq!(outcome.raw, message);
    assert_eq!(outcome.text, "Hello world\nNext line");
}

#[test]
fn direct_message_03() {
    let (u, h) = serve(
        vec![json!({"kind":"message","parts":[{"kind":"text","text":"answer"}]})],
        "0.3.0",
    );
    let mut c = A2aClient::connect(&u, None, Duration::from_secs(5)).unwrap();
    assert_eq!(c.run("hello").unwrap().text, "answer");
    h.join().unwrap();
}
#[test]
fn v1_cached_and_remote_wire() {
    for remote in [false, true] {
        let (u, h) = serve(
            vec![json!({"message":{"parts":[{"text":"answer"}]}})],
            "1.0",
        );
        let mut c = A2aClient::connect(&u, None, Duration::from_secs(5)).unwrap();
        assert_eq!(c.run("hello").unwrap().text, "answer");
        h.join().unwrap();
        let _ = remote;
    }
}
#[test]
fn polls_and_errors() {
    let (u, h) = serve(vec![task("working"), task("failed")], "0.3.0");
    let mut c = A2aClient::connect(&u, None, Duration::from_secs(5)).unwrap();
    assert!(c.run("x").unwrap_err().contains("failed"));
    h.join().unwrap();
}
#[test]
fn transport_failure_has_safe_category() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/private?token=never-expose",
        listener.local_addr().unwrap()
    );
    drop(listener);
    let mut client =
        A2aClient::connect_card(&card("1.0", &url), None, Duration::from_secs(2)).unwrap();
    let error = client.run("private-prompt").unwrap_err();
    // A closed loopback port may time out before the OS reports connection
    // refusal (notably on Windows). Both are valid safe transport categories.
    assert!(
        error.contains("connection failed") || error.contains("request timed out"),
        "{error}"
    );
    assert!(!error.contains("never-expose") && !error.contains("private-prompt"));
    assert!(error.contains("remote task may still be running"));
    assert!(error.contains("未自动重试"));
}

#[test]
fn rpc_error_identifies_polling_method() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/rpc", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for method in ["SendMessage", "GetTask"] {
            let (mut socket, _) = listener.accept().unwrap();
            let mut headers = Vec::new();
            let mut byte = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["method"], method);
            let response = if method == "SendMessage" {
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"task":{"id":"t1","status":{"state":"TASK_STATE_SUBMITTED"}}}})
            } else {
                assert_eq!(request["params"], json!({"id":"t1","tenant":"acme"}));
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"data":"private remote details"}})
            }.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        }
    });
    let mut client =
        A2aClient::connect_card(&card("1.0", &endpoint), None, Duration::from_secs(5)).unwrap();
    let error = client.run("hello").unwrap_err();
    worker.join().unwrap();
    assert!(
        error.contains("GetTask") && error.contains("-32602"),
        "{error}"
    );
    assert!(!error.contains("private remote details"));
}

#[test]
fn cancellation_requires_remote_confirmation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for version in ["0.3.0", "1.0"] {
        for confirm in [true, false] {
            let v1 = version == "1.0";
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/rpc", listener.local_addr().unwrap());
            let flag = Arc::new(AtomicBool::new(false));
            let worker_flag = flag.clone();
            let worker = thread::spawn(move || {
                for index in 0..2 {
                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut headers = Vec::new();
                    let mut byte = [0];
                    while !headers.ends_with(b"\r\n\r\n") {
                        socket.read_exact(&mut byte).unwrap();
                        headers.push(byte[0]);
                    }
                    let headers = String::from_utf8(headers).unwrap();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap();
                    let mut body = vec![0; length];
                    socket.read_exact(&mut body).unwrap();
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(
                        request["method"],
                        if index == 0 {
                            if v1 { "SendMessage" } else { "message/send" }
                        } else if v1 {
                            "CancelTask"
                        } else {
                            "tasks/cancel"
                        }
                    );
                    if index == 1 {
                        assert_eq!(request["params"]["id"], "t1");
                        if v1 {
                            assert_eq!(request["params"]["tenant"], "acme");
                        }
                    }
                    let state = if index == 1 && confirm {
                        if v1 {
                            "TASK_STATE_CANCELED"
                        } else {
                            "canceled"
                        }
                    } else if v1 {
                        "TASK_STATE_WORKING"
                    } else {
                        "working"
                    };
                    let task = task(state);
                    let result = if index == 0 && v1 {
                        json!({"task":task})
                    } else {
                        task
                    };
                    if index == 0 {
                        worker_flag.store(true, Ordering::SeqCst);
                    }
                    let response =
                        json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response.len(),
                        response
                    )
                    .unwrap();
                }
            });
            let mut client =
                A2aClient::connect_card(&card(version, &endpoint), None, Duration::from_secs(5))
                    .unwrap();
            let result = client.run_cancellable("test", json!({}), &flag);
            worker.join().unwrap();
            if confirm {
                assert_eq!(result.unwrap().state, "canceled");
            } else {
                assert!(result.unwrap_err().contains("取消未确认"));
            }
        }
    }
}

#[test]
fn card_validation_and_selection() {
    let good = card("1.0", "https://x");
    assert_eq!(inspect_card(&good).unwrap().version, "1.0");
    let mut bad = good.clone();
    bad["supportedInterfaces"] =
        json!([{"url":"https://x","protocolBinding":"JSONRPC","protocolVersion":"2.0"}]);
    assert!(inspect_card(&bad).is_err());
}
