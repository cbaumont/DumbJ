use std::io::{BufReader, PipeWriter};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

use crate::frame::{read_frame, write_frame};
use crate::{Server, ServerEvent};

pub(crate) const WAIT: Duration = Duration::from_secs(5);

pub(crate) struct Mock {
    pub(crate) incoming: Receiver<Value>,
    to_client: PipeWriter,
}

impl Mock {
    pub(crate) fn recv(&self) -> Value {
        self.incoming.recv_timeout(WAIT).unwrap()
    }

    pub(crate) fn send(&mut self, value: &Value) {
        write_frame(&mut self.to_client, &serde_json::to_vec(value).unwrap()).unwrap();
    }

    pub(crate) fn reply(&mut self, id: &Value, result: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub(crate) fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }
}

pub(crate) fn mock_pair(on_event: impl Fn(ServerEvent) + Send + 'static) -> (Server, Mock) {
    let (client_in, to_client) = std::io::pipe().unwrap();
    let (from_client, client_out) = std::io::pipe().unwrap();
    let server = Server::connect(client_in, client_out, on_event);
    let (tx, incoming) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from_client);
        while let Ok(Some(body)) = read_frame(&mut reader) {
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                break;
            }
        }
    });
    (
        server,
        Mock {
            incoming,
            to_client,
        },
    )
}
