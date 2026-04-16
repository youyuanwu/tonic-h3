use http::Uri;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let client_endpoint = tonic_h3_test::make_test_quinn_client_endpoint();

    let uri: Uri = "https://127.0.0.1:5047".parse().unwrap();
    let cc = h3_util::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let channel = h3_util::client::H3Channel::new(cc, uri.clone(), None);

    tracing::debug!("making greeter client.");
    let mut join_set = JoinSet::new();
    for _ in 0..2 {
        {
            let channel = channel.clone();
            join_set.spawn(async move {
                let mut client = tonic_h3_test::greeter_client::GreeterClient::new(channel);

                tracing::debug!("sending request.");
                {
                    let request = tonic::Request::new(tonic_h3_test::HelloRequest {
                        name: "Tonic".into(),
                    });
                    let response = client.say_hello(request).await.unwrap();

                    tracing::debug!("RESPONSE={:?}", response);
                }
            });
        }
    }
    join_set.join_all().await;
}
