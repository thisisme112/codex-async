use super::*;
use crate::legacy_core::config::ConfigBuilder;
use crate::*;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::time::Duration;
use tempfile::TempDir;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientMode {
    Interactive,
    ModelSelected,
    Ordinary,
    CodeModeDisabled,
}

async fn completed(
    client: &mut InProcessAppServerClient,
    thread_id: &str,
) -> color_eyre::Result<String> {
    Ok(tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = client.next_event().await.expect("client remains connected");
            if let InProcessServerEvent::ServerNotification(notification) = event
                && let ServerNotification::TurnCompleted(completed) = *notification
            {
                assert_eq!(completed.thread_id, thread_id);
                assert!(completed.turn.error.is_none(), "{:?}", completed.turn.error);
                return completed.turn.id;
            }
        }
    })
    .await?)
}

async fn submit(
    client: &InProcessAppServerClient,
    thread_id: &str,
    text: &str,
) -> color_eyre::Result<String> {
    let response: TurnStartResponse = client
        .request_typed(ClientRequest::TurnStart {
            request_id: RequestId::String(text.to_string()),
            params: TurnStartParams {
                thread_id: thread_id.to_string(),
                input: vec![UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        })
        .await?;
    Ok(response.turn.id)
}

#[tokio::test]
async fn embedded_client_completion_wake_is_default_and_ordinary_client_stays_opted_out()
-> color_eyre::Result<()> {
    // The embedded client resolves its Code Mode host beside the executable.
    // Run this test in an isolated fixture layout, without writing to Cargo/Bazel
    // output directories or mutating the parent process environment.
    const CHILD: &str = "CODEX_TEST_EMBEDDED_COMPLETION_CLIENT";
    if std::env::var_os(CHILD).is_none() {
        let layout = TempDir::new()?;
        let executable = std::env::current_exe()?;
        let child = layout
            .path()
            .join(executable.file_name().expect("test executable name"));
        let host = codex_utils_cargo_bin::cargo_bin("codex-code-mode-host")?;
        std::fs::copy(&executable, &child)?;
        std::fs::copy(
            &host,
            layout.path().join(format!(
                "codex-code-mode-host{}",
                std::env::consts::EXE_SUFFIX
            )),
        )?;
        let output = tokio::process::Command::new(child)
            .args(["--exact", "code_mode_startup::tests::embedded_client_completion_wake_is_default_and_ordinary_client_stays_opted_out", "--nocapture"])
            .env(CHILD, "1")
            .output().await?;
        assert!(
            output.status.success(),
            "embedded client fixture failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }

    for mode in [
        ClientMode::Interactive,
        ClientMode::ModelSelected,
        ClientMode::Ordinary,
        ClientMode::CodeModeDisabled,
    ] {
        let server = responses::start_mock_server().await;
        let home = TempDir::new()?;
        app_test_support::write_mock_responses_config_toml(
            home.path(),
            &server.uri(),
            &BTreeMap::from([(
                Feature::CodeMode,
                matches!(mode, ClientMode::Interactive | ClientMode::Ordinary),
            )]),
            /*auto_compact_limit*/ 100_000,
            /*requires_openai_auth*/ None,
            "mock_provider",
            "compact",
        )?;
        if mode == ClientMode::ModelSelected {
            let mut catalog = codex_models_manager::bundled_models_response()?;
            catalog.models.truncate(1);
            let model = &mut catalog.models[0];
            model.slug = "test-gpt-5.1-codex".to_string();
            model.tool_mode = Some(codex_protocol::openai_models::ToolMode::CodeModeOnly);
            model.use_responses_lite = false;
            let catalog_path = home.path().join("models.json");
            std::fs::write(&catalog_path, serde_json::to_vec(&catalog)?)?;
            let config_path = home.path().join("config.toml");
            let configured = std::fs::read_to_string(&config_path)?;
            let catalog_path = toml::Value::String(catalog_path.display().to_string());
            std::fs::write(
                config_path,
                format!("model_catalog_json = {catalog_path}\n{configured}"),
            )?;
        }
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .fallback_cwd(Some(home.path().to_path_buf()))
            .build()
            .await?;
        // No CLI overrides: config- and model-selected Code Mode both need the
        // interactive capability, including a later model switch on a live thread.
        let endpoint = RemoteAppServerEndpoint::UnixSocket {
            socket_path: AbsolutePathBuf::from_absolute_path_checked(
                home.path().join("daemon.sock"),
            )?,
        };
        let default_target = app_server_target_for_launch(
            /*explicit_remote_endpoint*/ None,
            Some(AbsolutePathBuf::from_absolute_path_checked(
                home.path().join("daemon.sock"),
            )?),
            can_reuse_implicit_local_daemon(
                &[],
                &LoaderOverrides::default(),
                /*strict_config*/ false,
                /*has_non_replayable_launch_overrides*/ false,
            ),
            /*workload_identity_selected*/ false,
            /*exec_server_url*/ None,
        )?;
        let mut cli = <Cli as clap::Parser>::parse_from(["codex"]);
        assert_eq!(
            interactive_target(default_target.clone(), &cli),
            AppServerTarget::Embedded
        );
        cli.agents_overview = true;
        assert_eq!(
            interactive_target(default_target.clone(), &cli),
            default_target
        );
        cli.agents_overview = false;
        let explicit = AppServerTarget::Remote { endpoint };
        assert_eq!(interactive_target(explicit.clone(), &cli), explicit);
        assert_eq!(
            interactive_target(AppServerTarget::Cloud, &cli),
            AppServerTarget::Cloud
        );

        let mut client = if mode == ClientMode::Ordinary {
            let state_db =
                init_state_db_for_app_server_target(&config, &AppServerTarget::Embedded).await?;
            start_embedded_app_server_with(
                Arg0DispatchPaths::default(),
                config,
                Vec::new(),
                LoaderOverrides::default(),
                /*strict_config*/ false,
                CloudConfigBundleLoader::default(),
                codex_feedback::CodexFeedback::new(),
                /*log_db*/ None,
                state_db,
                Arc::new(EnvironmentManager::default_for_tests()),
                codex_app_server_client::EmbeddedNetworkPolicy::default(),
                InProcessAppServerClient::start,
            )
            .await?
        } else {
            crate::tests::start_test_embedded_app_server(config).await?
        };
        let thread: ThreadStartResponse = client
            .request_typed(ClientRequest::ThreadStart {
                request_id: RequestId::String("thread".to_string()),
                params: ThreadStartParams {
                    model: Some("test-gpt-5.1-codex".to_string()),
                    // This isolated TUI harness has no argv0 sandbox helper.
                    sandbox: (mode == ClientMode::CodeModeDisabled)
                        .then_some(codex_app_server_protocol::SandboxMode::DangerFullAccess),
                    ..Default::default()
                },
            })
            .await?;
        let thread_id = thread.thread.id;
        if mode == ClientMode::CodeModeDisabled {
            let request =
                responses::mount_sse_once(&server, responses::sse_completed("no-code-mode")).await;
            let turn = submit(&client, &thread_id, "ordinary text").await?;
            assert_eq!(completed(&mut client, &thread_id).await?, turn);
            assert!(
                !request.single_request().body_json()["tools"]
                    .as_array()
                    .expect("tools")
                    .iter()
                    .any(|tool| tool["name"] == "exec")
            );
            #[cfg(unix)]
            {
                responses::mount_sse_once(&server, responses::sse(vec![
                    responses::ev_function_call("direct", "exec_command", r#"{"cmd":"sleep 1.5; printf client-command-result","yield_time_ms":250,"login":false}"#),
                    responses::ev_completed("command"),
                ])).await;
                let yielded =
                    responses::mount_sse_once(&server, responses::sse_completed("command-yielded"))
                        .await;
                let first = submit(&client, &thread_id, "run a background command").await?;
                assert_eq!(completed(&mut client, &thread_id).await?, first);
                let output = yielded
                    .single_request()
                    .function_call_output("direct")
                    .to_string();
                let id = output
                    .split("Process running with session ID ")
                    .nth(1)
                    .unwrap_or_else(|| panic!("running command: {output}"))
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>();
                let wake = responses::mount_sse_once(&server, responses::sse(vec![
                    responses::ev_function_call("read-command", "write_stdin", &serde_json::json!({"session_id": id.parse::<i32>()?, "chars":"", "yield_time_ms":1000}).to_string()),
                    responses::ev_completed("command-wake"),
                ])).await;
                let result =
                    responses::mount_sse_once(&server, responses::sse_completed("command-read"))
                        .await;
                tokio::time::sleep(Duration::from_millis(/*millis*/ 100)).await;
                assert!(wake.requests().is_empty());
                assert_ne!(completed(&mut client, &thread_id).await?, first);
                assert!(
                    wake.single_request()
                        .body_contains_text("<exec_completion>")
                );
                assert!(
                    result
                        .single_request()
                        .function_call_output("read-command")
                        .to_string()
                        .contains("client-command-result")
                );
            }
            client.shutdown().await?;
            continue;
        }
        responses::mount_sse_once(&server, responses::sse(vec![
            responses::ev_custom_tool_call("exec-1", "exec", "yield_control(); await new Promise(resolve => setTimeout(resolve, 1500)); text('client retained result');"),
            responses::ev_completed("execute"),
        ])).await;
        let yielded = responses::mount_sse_once(&server, responses::sse_completed("yielded")).await;
        let first = submit(&client, &thread_id, "run and finish while cell runs").await?;
        assert_eq!(completed(&mut client, &thread_id).await?, first);
        let output = yielded.single_request().custom_tool_call_output("exec-1");
        let output = &output["output"];
        let text = output
            .as_str()
            .or_else(|| output[0]["text"].as_str())
            .expect("yielded text");
        let cell_id = text
            .strip_prefix("Script running with cell ID ")
            .expect("running cell")
            .split('\n')
            .next()
            .expect("cell ID");
        assert_eq!(
            yielded
                .single_request()
                .body_contains_text("can resume automatically once idle"),
            mode != ClientMode::Ordinary
        );
        let wake = responses::mount_sse_once(
            &server,
            responses::sse(vec![
                responses::ev_function_call(
                    "wait-1",
                    "wait",
                    &serde_json::json!({"cell_id": cell_id, "yield_time_ms": 1}).to_string(),
                ),
                responses::ev_completed("wait"),
            ]),
        )
        .await;
        let result =
            responses::mount_sse_once(&server, responses::sse_completed("retrieved")).await;
        tokio::time::sleep(Duration::from_millis(/*millis*/ 100)).await;
        assert!(wake.requests().is_empty(), "no polling before readiness");
        if mode == ClientMode::Ordinary {
            tokio::time::sleep(Duration::from_millis(1600)).await;
            assert!(
                wake.requests().is_empty(),
                "ordinary client must not auto-start"
            );
            submit(&client, &thread_id, "retrieve manually").await?;
        }
        let resumed = completed(&mut client, &thread_id).await?;
        assert_ne!(resumed, first);
        assert_eq!(
            wake.single_request()
                .body_contains_text("<code_mode_completion>"),
            mode != ClientMode::Ordinary
        );
        assert!(
            !wake
                .single_request()
                .message_input_texts("user")
                .iter()
                .any(|text| text.contains("client retained result"))
        );
        assert!(
            result
                .single_request()
                .function_call_output("wait-1")
                .to_string()
                .contains("client retained result")
        );
        tokio::time::sleep(Duration::from_millis(/*millis*/ 100)).await;
        assert_eq!(wake.requests().len(), 1);
        client.shutdown().await?;
    }
    Ok(())
}
