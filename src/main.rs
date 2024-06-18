use http_server_starter_rust::{handle_request, RequestParser, ARGUMENTS};
use once_cell::sync::Lazy;
use tokio::{
    io::{BufReader, BufWriter},
    net::{TcpListener, TcpStream},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Lazy::force(&ARGUMENTS);
    let listener = TcpListener::bind("127.0.0.1:4221").await?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    handle_connection(stream)
                        .await
                        .inspect_err(|err| eprintln!("{err}"))
                });
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}

async fn handle_connection(stream: TcpStream) -> anyhow::Result<()> {
    let (reader, mut writer) = {
        let (reader, writer) = stream.into_split();
        let reader = BufReader::new(reader);
        let writer = BufWriter::new(writer);
        (reader, writer)
    };

    let request = RequestParser::parse(reader).await?;

    handle_request(request, &mut writer).await?;
    Ok(())
}
