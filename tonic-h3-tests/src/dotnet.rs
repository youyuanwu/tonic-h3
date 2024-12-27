use std::path::Path;

async fn invoke_csharp_client(root_dir: &Path) {
    // send csharp request to server
    println!("launching csharp client");
    let child_client = std::process::Command::new("dotnet")
        .current_dir(root_dir)
        .args(["run", "--project", "./dotnet/client/client.csproj"])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("Couldn't run client");
    assert!(child_client.status.success());
}

fn run_example_server() -> std::process::Child {
    println!("launching rust server");
    std::process::Command::new("cargo")
        .args([
            "run",
            "--package",
            "tonic-h3-test",
            "--example",
            "server",
            "--",
            "--nocapture",
        ])
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn dotnet_client() {
    // run server example
    let curr_dir = std::env::current_dir().unwrap();
    println!("{:?}", curr_dir);
    let root_dir = curr_dir.parent().unwrap();

    let mut h_svr = run_example_server();

    // send client request
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    invoke_csharp_client(root_dir).await;

    h_svr.kill().unwrap();

    let server_out = h_svr.wait_with_output().unwrap();
    // server kill may exit with code 1.
    println!("server output: {:?}", server_out);
}
