mod client;
mod server;

use color_eyre::Result;
use std::sync::Arc;
use tokio;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} [server|client]", args[0]);
        std::process::exit(1);
    }
    match args[1].as_str() {
        "server" => {
            if args.len() < 5 {
                eprintln!(
                    "Usage: {} server <id> <tcp_address> <unix_socket_path>",
                    args[0]
                );
                std::process::exit(1);
            }
            let server_id: u32 = args[2].parse()?;
            let tcp_port: u16 = args[3].parse()?;
            let unix_socket = args[4].clone();
            let tcp_addr = format!("127.0.0.1:{tcp_port}");
            let handoff_target = args.get(5).cloned();
            println!("Starting Server {server_id} on {tcp_addr}");
            println!("Unix socket: {}", unix_socket);

            let server = Arc::new(server::Server::new(
                server_id,
                &tcp_addr,
                unix_socket,
                handoff_target,
            )?);
            server.run().await?;
        }
        "client" => {
            if args.len() < 3 {
                eprintln!("Usage: {} client <tcp_addr>", args[0]);
                std::process::exit(1);
            }
            let server_addr = args[2].clone();
            let client = client::Client::new(server_addr);
            client.run().await?;
        }
        _ => {
            eprintln!("Unknown mode: {}", args[1]);
            std::process::exit(1);
        }
    }

    Ok(())
}
