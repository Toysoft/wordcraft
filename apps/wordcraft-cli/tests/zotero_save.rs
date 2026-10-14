//! A failed/cancelled Zotero operation must not create or overwrite --save output (#1330).
use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use wordcraft_zotero::wire;

#[test]
fn only_successful_zotero_operations_save() {
    for ending in ["cancel", "error", "broken-frame", "success"] {
        for existing in [false, true] {
            let dir = std::env::temp_dir().join(format!("wordcraft-zotero-save-{}-{ending}-{existing}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let input = dir.join("input.txt");
            let output = if existing { input.clone() } else { dir.join("output.txt") };
            std::fs::write(&input, "original").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port().to_string();
            let server = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "CLI did not connect");
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => panic!("accept: {e}"),
                    }
                };
                // macOS inherits the listener's nonblocking flag on accepted sockets.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
                wire::read_frame(&mut stream).unwrap().unwrap();
                let mut calls = vec![json!(["Application_getActiveDocument", [3]]), json!(["Document_insertText", [1, "changed "]])];
                if ending == "error" {
                    calls.push(json!(["Not_a_method", []]));
                }
                if matches!(ending, "error" | "success") {
                    calls.push(json!(["Document_complete", [1]]));
                }
                for (i, call) in calls.iter().enumerate() {
                    wire::write_frame(&mut stream, i as u32 + 1, &serde_json::to_vec(call).unwrap()).unwrap();
                    wire::read_frame(&mut stream).unwrap().unwrap();
                }
                if ending == "broken-frame" {
                    use std::io::Write;
                    stream.write_all(&[0, 0]).unwrap();
                }
            });
            let result = Command::new(env!("CARGO_BIN_EXE_wordcraft-cli"))
                .args(["zotero", "refresh"])
                .arg(&input)
                .args(["--port", &port, "--save"])
                .arg(&output)
                .output()
                .unwrap();
            server.join().unwrap();
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.success(), ending == "success", "{ending}: {stderr}");
            if ending == "success" {
                assert!(std::fs::read_to_string(&output).unwrap().contains("changed original"));
            } else if existing {
                assert_eq!(std::fs::read_to_string(&output).unwrap(), "original", "{ending}: overwrote input");
            } else {
                assert!(!output.exists(), "{ending}: created output");
            }
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
