// Sync server for Trip Planning

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::{env::args, sync::Arc};
use std::io::Error;

use loro::LoroDoc;
use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::{select, spawn};
use tokio::sync::mpsc::{self, Sender};
use tokio_tungstenite::accept_hdr_async;

use futures_util::{future, SinkExt, StreamExt, TryStreamExt};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::Message;

type ClientMap = Arc<Mutex<HashMap<SocketAddr, Sender<WsMessage>>>>;
type LoroMutex = Arc<Mutex<LoroDoc>>;

// WS message
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type")]
enum WsMessage {
    Sync {
        random: i32
    },
    ApplyDelta { asd: i32 }
}

impl TryFrom<&[u8]> for WsMessage {
    type Error = rmp_serde::decode::Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        // Deserialize
        let mut des = Deserializer::new(value);
        WsMessage::deserialize(&mut des)
    }
}

impl TryFrom<&WsMessage> for Vec<u8> {
    type Error = rmp_serde::encode::Error;
    fn try_from(value: &WsMessage) -> Result<Self, Self::Error> {
        // Serialize
        let mut buf = Self::new();
        let mut ser = Serializer::new(&mut buf);
        value.serialize(&mut ser)?;
        Ok(buf)
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Get the addr to bind on
    let addr = args().nth(1).unwrap_or("127.0.0.1:8080".to_string());   

    // TODO: better arg parsing + CORS Origin allowlist
    
    // Start listening
    let sock = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on {addr}");

    let client_map = ClientMap::new(Mutex::new(HashMap::new()));
    let loro_doc = LoroMutex::new(Mutex::new(LoroDoc::new()));

    // Main server loop
    while let Ok((stream, remote_addr)) = sock.accept().await {
        // Handle connection
        spawn(handle_connection(client_map.clone(), loro_doc.clone(), remote_addr, stream));
    }

    Ok(())
}

async fn handle_connection(client_map: ClientMap, loro_doc: LoroMutex, addr: SocketAddr, stream: TcpStream) -> Result<(), String> {
    // Origin validation callback
    let callback = |req: &Request, res: Response| {
        // Make sure the origin is valid
        // TODO: don't hardcode
        if req.headers().get("origin") == Some(&HeaderValue::from_static("http://localhost:5173")) {
            Ok(res)
        } else {
            let mut error_res = ErrorResponse::new(Some("Invalid `Origin` header".into()));
            *error_res.status_mut() = StatusCode::FORBIDDEN;
            Err(error_res)
        }
    };

    let ws_stream = accept_hdr_async(
        stream,
        callback
    ).await.map_err(|e| e.to_string())?;

    println!("{} connected", &addr);

    // Create a channel to receive data from other clients
    let (tx, mut rx) = mpsc::channel(8);

    // Add it to the map
    client_map.lock().unwrap().insert(addr, tx);

    let (mut write, read) = ws_stream.split();

    let parse_incoming = read.try_for_each(|msg: Message| {
        if let Message::Binary(data) = msg {
            // Deserialize the message
            if let Ok(msg) = data.as_slice().try_into() {
                match msg {
                    WsMessage::Sync { random } => {
                        println!("sync {random}")
                    }
                    WsMessage::ApplyDelta { asd } => {
                        println!("apply delta")
                    }
                }
            }
        }

        future::ok(())
    });

    let send_from_others = spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(data) = (&msg).try_into() {
                let _ = write.send(Message::Binary(data)).await;
            }
        }
    });

    select! {
        _ = parse_incoming => {},
        _ = send_from_others => {}
    }

    client_map.lock().unwrap().remove(&addr);

    println!("{} disconnected", &addr);

    Ok(())
}
