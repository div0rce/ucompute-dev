//! WP-04 C2, REAL and SINGLE-HOST: stories15M split between a plaintext loopback `ggml-rpc-server`
//! (a loopback control, tests only: spec section 28.4) and this host's GPU, through the adapter's
//! split branch (configuration P, ADR-0005 D2). Two processes on one host: this never satisfies a
//! two-machine criterion (F-03-01, F-03-02, Gate D) and does not exercise Common's transport.

mod common;

use common::*;
use common_runtime::llamacpp::{process, split};
use common_runtime::*;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The pinned `ggml-rpc-server` on `127.0.0.1:<port>`; killed and reaped on drop, panics included.
struct RpcServer {
    child: Child,
    port: u16,
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_rpc_server(engine_dir: &Path, log: &Path) -> RpcServer {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let out = std::fs::File::create(log).unwrap();
    let child = Command::new(common_artifacts::backend::binary(engine_dir, "ggml-rpc-server"))
        .args(["-H", "127.0.0.1", "-p", &port.to_string()])
        .env_clear()
        .envs(process::clean_environment(std::env::vars_os()))
        .env("GGML_RPC_NO_RDMA", "1")
        .stdin(Stdio::null())
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .spawn()
        .unwrap();
    let mut server = RpcServer { child, port };
    // readiness: the banner names the loopback endpoint, then the port accepts (a test-only probe;
    // ADR-0005 D3 replaces it with a socket-owner check for a real stage host)
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            server.child.try_wait().unwrap().is_none(),
            "rpc-server exited:\n{}",
            process::read_log(log)
        );
        let banner = process::read_log(log);
        if banner.contains(&format!("endpoint       : 127.0.0.1:{port}"))
            && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        {
            return server;
        }
        assert!(Instant::now() < deadline, "rpc-server not ready:\n{banner}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The first GPU in `llama-server --list-devices` (`MTL0`, `Vulkan0`, ...), None on a CPU-only host.
fn gpu_device(engine_dir: &Path) -> Option<String> {
    let out = Command::new(common_artifacts::backend::binary(engine_dir, "llama-server"))
        .arg("--list-devices")
        .env_clear()
        .envs(process::clean_environment(std::env::vars_os()))
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .skip_while(|l| !l.starts_with("Available devices"))
        .skip(1)
        .filter_map(|l| l.trim().split_once(':').map(|(d, _)| d.to_string()))
        .find(|d| !d.starts_with("BLAS") && !d.starts_with("CPU") && !d.starts_with("RPC"))
}

async fn greedy_ids(loaded: &dyn LoadedModel) -> (String, Vec<u32>, String) {
    let (text, ids, _, _, finish) = collect(
        loaded
            .generate(greedy(64), tokio_util::sync::CancellationToken::new())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    (text, ids, finish)
}

#[tokio::test]
async fn split_load_places_layers_as_planned_single_host() {
    let Some((backend, model)) = setup() else { return };
    let Some(gpu) = gpu_device(&backend.engine_dir) else {
        eprintln!("SKIP: no GPU device on this host; configuration P needs one");
        return;
    };
    let n_layer = common_models::gguf::read_metadata(&model).unwrap().block_count;
    let k = n_layer / 2;
    let logs = tempfile::tempdir().unwrap();
    let rpc = start_rpc_server(&backend.engine_dir, &logs.path().join("rpc-server.log"));

    // configuration P: the RPC stage holds blocks [0,k), the GPU [k,n) plus the output layer
    let stages = vec![
        SplitStage {
            device: "RPC0".into(),
            first_block: 0,
            end_block: k,
        },
        SplitStage {
            device: gpu.clone(),
            first_block: k,
            end_block: n_layer,
        },
    ];
    let mut req = load_req(&model, DevicePreference::Gpu);
    req.split = Some(Split {
        rpc_ports: vec![rpc.port],
        stages: stages.clone(),
        n_layer,
        no_mmap: false,
        engine_pid: Default::default(),
    });
    let loaded = backend.load_llama(req).await.unwrap();
    let info = loaded.info().clone();
    assert_eq!(info.split.as_ref(), Some(&stages), "stages as proved");
    assert_eq!(info.fallback_reason, None);
    assert_eq!(
        info.device_actual,
        format!("RPC0 (127.0.0.1:{}), {gpu}", rpc.port),
        "the engine's devices, from its log"
    );
    let log = process::read_log(&loaded.log_path);
    let proof = split::parse_buffers(&log);
    let plan = [("RPC0", 0, k as usize), (gpu.as_str(), k as usize, n_layer as usize)];
    assert_eq!(split::check_buffers(&proof, &plan, n_layer as usize), Ok(()));

    let (text, ids, finish) = greedy_ids(loaded.as_ref()).await;
    assert!(
        !text.trim().is_empty() && ids.len() == 64 && finish == "length",
        "{finish}: {text:?}"
    );
    // After a generation, so a prompt or reply the engine logged would be there (-lv 4 logs neither).
    let log = process::read_log(&loaded.log_path);
    assert!(!log.contains(PROMPT), "-lv 4: no prompt text in the engine log");
    assert!(!log.contains(text.trim()), "-lv 4: no reply text in the engine log");
    let split_pid = info.pid.unwrap();
    loaded.unload().await.unwrap();
    assert!(!process::alive(split_pid), "split engine stopped");

    let local = backend
        .load_llama(load_req(&model, DevicePreference::Gpu))
        .await
        .unwrap();
    let local_ms = local.info().load_ms;
    let (local_text, local_ids, _) = greedy_ids(local.as_ref()).await;
    local.unload().await.unwrap();
    let prefix = ids.iter().zip(&local_ids).take_while(|(a, b)| a == b).count();

    eprintln!(
        "SINGLE-HOST split (configuration P, stories15M, {n_layer} blocks): RPC0 [0,{k}) on rpc-server \
         127.0.0.1:{}, {gpu} [{k},{n_layer}) + output; KV MiB {:?}, model MiB {:?}; split load {:.0} ms, \
         local load {local_ms:.0} ms; {} tokens, finish {finish}; prefix match with local {prefix}/64",
        rpc.port,
        proof.kv_mib,
        proof.model_mib,
        info.load_ms,
        ids.len()
    );
    eprintln!("split text: {text:?}\nlocal text: {local_text:?}");
    // Token identity is claimed only where the lab showed it: Metal on both stages, 64/64 (REPORT
    // Addenda 9 and 11). Another backend pair is reported as measured (ADR-0005 D10).
    if gpu == "MTL0" {
        assert_eq!(ids, local_ids, "Metal+Metal split must equal local Metal");
    }

    let port = rpc.port;
    drop(rpc);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "no rpc-server left on 127.0.0.1:{port}"
    );
}
