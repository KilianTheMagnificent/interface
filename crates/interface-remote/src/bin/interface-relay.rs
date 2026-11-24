use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut bind: SocketAddr = "127.0.0.1:8080".parse()?;
    let mut mac_id = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--bind" => bind = args.next().ok_or("--bind requires an address")?.parse()?,
            "--mac-id" => mac_id = args.next(),
            "--help" => {
                println!("interface-relay --mac-id PUBLIC_MAC_ID [--bind 127.0.0.1:8080]");
                return Ok(());
            }
            _ => return Err("Unknown argument. Use --help.".into()),
        }
    }
    let router = interface_remote::relay::relay_router(mac_id.ok_or("--mac-id is required")?)?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("Interface relay listening on {}", listener.local_addr()?);
    // Each connection's address keeps one client from taking every waiting
    // socket (`relay_admission`).
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
