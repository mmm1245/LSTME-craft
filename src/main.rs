mod client;
mod common;
mod net;
mod server;

fn main() {
    std::thread::spawn(|| {
        server::server(6543);
    });
    client::client("127.0.0.1:6543", "mmm".to_string());
}
