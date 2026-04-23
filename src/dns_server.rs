use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr};

use eyre::{Result, WrapErr};
use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;

pub async fn run_dns_server(port: u16) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let socket = match UdpSocket::bind(addr).await {
        Ok(socket) => socket,
        Err(err) if err.kind() == ErrorKind::AddrInUse => {
            return Err(eyre::eyre!(
                "DNS UDP port {port} on 127.0.0.1 is already in use. Another NeoMist instance or local service is already bound to that port."
            ));
        }
        Err(err) => {
            return Err(err)
                .wrap_err_with(|| format!("Failed to bind DNS UDP socket on {addr}"));
        }
    };
    tracing::info!("DNS server listening on {addr}");

    let mut buf = [0u8; 512];
    loop {
        let (len, peer) = socket
            .recv_from(&mut buf)
            .await
            .wrap_err("Failed to receive DNS packet")?;
        let request = match Message::from_vec(&buf[..len]) {
            Ok(msg) => msg,
            Err(_) => continue,
        };

        let response = build_response(&request);
        let response_bytes = match response.to_vec() {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        let _ = socket.send_to(&response_bytes, peer).await;
    }
}

fn build_response(request: &Message) -> Message {
    let mut response = Message::new();
    response.set_id(request.id());
    response.set_message_type(MessageType::Response);
    response.set_op_code(request.op_code());
    response.set_recursion_desired(request.recursion_desired());
    response.set_recursion_available(false);
    response.set_authoritative(true);

    let mut answered = false;

    for query in request.queries() {
        let name = query.name().clone();
        response.add_query(query.clone());

        if query.query_type() != RecordType::A {
            continue;
        }

        if matches_zone(&name) {
            let record = Record::from_rdata(name, 60, RData::A(A(Ipv4Addr::LOCALHOST)));
            response.add_answer(record);
            answered = true;
        }
    }

    if !answered {
        response.set_response_code(ResponseCode::NXDomain);
    }

    response
}

fn matches_zone(name: &Name) -> bool {
    let value = name.to_utf8().to_lowercase();
    value.ends_with(".eth.") || value.ends_with(".wei.")
}
