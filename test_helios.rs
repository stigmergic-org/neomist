use helios::client::EthereumClientBuilder;
fn main() {
    let mut builder = EthereumClientBuilder::new();
    builder = builder.execution_rpcs(vec!["http://localhost".to_string()]);
}
