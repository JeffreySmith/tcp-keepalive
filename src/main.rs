mod client;
mod server;

use color_eyre::Result;
use std::{collections::HashSet, sync::Arc};

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Server {
        /// Use this to override the server id. By default, this will be generated for you
        #[arg(short, long)]
        id: Option<u32>,
        /// The folder where the socket files will be created. Default is /tmp
        /// One unix socket will be created per server
        #[arg(short, long, default_value = "/tmp")]
        unix_socket_folder: String,
        /// The port to listen on
        #[arg(short, long, default_value_t = 3000)]
        port: u16,
        #[arg(short, long, default_value = "127.0.0.1")]
        tcp_address: String,
    },
    Client {
        /// The server address to connect to (eg 127.0.0.1:3000)
        #[arg(short, long)]
        tcp_address: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Commands::Server {
            id,
            tcp_address,
            unix_socket_folder,
            port,
        } => {
            let server_id = if let Some(i) = id {
                i
            } else {
                find_available_server_id(&unix_socket_folder, port)?
            };
            let tcp_addr = format!("{tcp_address}:{port}");
            let server = Arc::new(server::Server::new(
                server_id,
                &tcp_addr,
                unix_socket_folder,
                None,
            )?);
            server.run().await?;
        }
        Commands::Client { tcp_address } => {
            let client = client::Client::new(tcp_address);
            client.run().await?;
        }
    }

    Ok(())
}

fn find_available_server_id(socket_folder: &str, port: u16) -> Result<u32> {
    let pattern = format!("socket-forward_{}_", port);
    let mut used_ids = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(socket_folder) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().into_string().unwrap_or_default();
            if file_name.starts_with(&pattern)
                && let Some(id_str) = file_name
                    .strip_prefix(&pattern)
                    .and_then(|s| s.split('.').next())
                && let Ok(id) = id_str.parse::<u32>()
            {
                used_ids.insert(id);
            }
        }
    }
    let mut available_id = 1;
    while used_ids.contains(&available_id) {
        available_id += 1;
    }
    Ok(available_id)
}
