#[tokio::main]
async fn main() {
    tonic_h3_test::try_setup_tracing();
    let client_endpoint = tonic_h3_test::make_test_client_endpoint();

    let uri = "https://127.0.0.1:5047".parse().unwrap();
    let channel = tonic_h3::quinn::new_quinn_h3_channel(uri, client_endpoint.clone());

    tracing::debug!("making greeter client.");
    let mut client = tonic_h3_test::greeter_client::GreeterClient::new(channel);

    tracing::debug!("sending request.");
    {
        let request = tonic::Request::new(tonic_h3_test::HelloRequest {
            name: "Tonic".into(),
        });
        let response = client.say_hello(request).await.unwrap();

        tracing::debug!("RESPONSE={:?}", response);
    }
}
