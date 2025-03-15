use std::path::{Path, PathBuf};

fn invoke_dotnet_client(root_dir: &Path) -> std::process::Output {
    // send csharp request to server
    println!("launching dotnet client");
    std::process::Command::new("dotnet")
        .current_dir(root_dir)
        .args(["run", "--project", "./dotnet/client/client.csproj"])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("Couldn't run client")
}

fn invoke_rust_client() {
    println!("launching rust client");
    let child_client = std::process::Command::new("cargo")
        .args([
            "run",
            "--package",
            "tonic-h3-test",
            "--example",
            "client",
            "--",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("Couldn't run client");
    assert!(child_client.status.success());
}

fn run_rust_server() -> std::process::Child {
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
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap()
}

fn run_dotnet_server(root_dir: &Path) -> std::process::Child {
    println!("launching rust server");
    std::process::Command::new("dotnet")
        .current_dir(root_dir)
        .args(["run", "--project", "./dotnet/server/server.csproj"])
        .spawn()
        .unwrap()
}

fn get_root_dir() -> PathBuf {
    let curr_dir = std::env::current_dir().unwrap();
    println!("{:?}", curr_dir);
    let root_dir = curr_dir.parent().unwrap();
    root_dir.to_path_buf()
}

#[tokio::test]
#[serial_test::serial]
async fn rust_server() {
    let root_dir = get_root_dir();
    // run server example
    let mut h_svr = run_rust_server();

    // send client request
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let mut ok = false;
    for _ in 0..30 {
        let out = invoke_dotnet_client(root_dir.as_path());
        if out.status.success() {
            ok = true;
            break;
        } else {
            println!("retrying invoke_dotnet_client");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    assert!(ok);

    invoke_rust_client();

    h_svr.kill().unwrap();

    let server_out = h_svr.wait_with_output().unwrap();
    // server kill may exit with code 1.
    println!("server output: {:?}", server_out);
}

#[tokio::test]
#[serial_test::serial]
async fn dotnet_server() {
    let root_dir = get_root_dir();
    // run server example
    let mut h_svr = run_dotnet_server(root_dir.as_path());

    // server start might be slow
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // send client request
    let mut ok = false;
    for _ in 0..30 {
        let out = invoke_dotnet_client(root_dir.as_path());
        if out.status.success() {
            ok = true;
            break;
        } else {
            println!("retrying invoke_dotnet_client");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    assert!(ok);

    invoke_rust_client();

    h_svr.kill().unwrap();

    let server_out = h_svr.wait_with_output().unwrap();
    // server kill may exit with code 1.
    println!("server output: {:?}", server_out);
}
