use tokio::io::BufReader;
use tokio::net::TcpStream;
//use tokio::io::{AsyncReadExt, AsyncWriteExt};
use color_eyre::Result;
use tokio::io::AsyncBufReadExt;

pub struct Client {
    server_address: String,
}

impl Client {
    pub fn new(server_address: String) -> Self {
        Client { server_address }
    }
    pub async fn run(&self) -> Result<()> {
        let stream = TcpStream::connect(&self.server_address).await?;
        println!("Connected to server at {}", self.server_address);

        println!("[Client] Connected! Peer: {:?}", stream.peer_addr()?);

        let reader = BufReader::new(stream);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            println!("[Client] Received: {}", line);
        }
        println!("[Client] Connection closed by server");

        Ok(())
    }
}
