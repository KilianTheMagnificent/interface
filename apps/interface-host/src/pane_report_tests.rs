//! Full native pane MCP path with real PTYs and simulated provider/report CLIs.
use super::*;
use crate::pane_mcp::{PANE_MCP_SOCKET_ENV, PaneMcpServer, pane_mcp_socket_path};

struct ReportSessions;
impl TerminalProviderSessionLookup for ReportSessions {
    fn begin_capture(
        &self,
        _: TerminalLaunchProfileId,
        _: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }
    fn record_process(
        &self,
        _: TerminalLaunchProfileId,
        _: &str,
        _: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }
    fn current_session(
        &self,
        profile: TerminalLaunchProfileId,
        _: &TerminalProcessIdentity,
        _: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(Some(TerminalProviderSession {
            id: if profile == TerminalLaunchProfileId::Claude {
                "11111111-1111-4111-8111-111111111111"
            } else {
                "01a06e8d-c9ac-78e3-be73-19f2255b8932"
            }
            .into(),
            transition: TerminalProviderSessionTransition::Resume,
        }))
    }
}
const CLIENT: &str = r"import json, os, pathlib, socket, sys, time, traceback
root, provider = pathlib.Path(sys.argv[1]), sys.argv[2]
def save(name, value):
    (root / (provider + '-' + name)).write_text(json.dumps(value))
def call(name, args, legacy=False):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(10)
    legacy = legacy or name == 'notice'
    path = pathlib.Path(os.environ['INTERFACE_PANE_MCP_SOCKET'])
    s.connect(str(path if legacy else path.with_name('cheap_agents.sock')))
    stream = s.makefile('rw')
    stream.write(json.dumps({'jsonrpc':'2.0','id':1,'method':'initialize','params':{}})+'\n'); stream.flush()
    initialized = json.loads(stream.readline())
    assert (initialized['result']['instructions'] == '') if legacy else ('Optional read-only workers' in initialized['result']['instructions'])
    assert initialized['result']['serverInfo']['name'] == ('iface' if legacy else 'cheap_agents')
    stream.write(json.dumps({'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}})+'\n'); stream.flush()
    names = [tool['name'] for tool in json.loads(stream.readline())['result']['tools']]
    assert names == (['pass_off','read_pane','send_to_pane'] if legacy else ['worker_models','worker_start','worker_collect','worker_followup','worker_cancel','worker_review']), names
    method = 'interface/worker-notice' if name == 'notice' else 'tools/call'
    params = {} if name == 'notice' else {'name':name,'arguments':args}
    stream.write(json.dumps({'jsonrpc':'2.0','id':3,'method':method,'params':params})+'\n'); stream.flush()
    response = json.loads(stream.readline())
    stream.close(); s.close()
    if 'error' in response: return {'error':response['error']}
    result = response['result']
    return {'error':result['content'][0]['text']} if result.get('isError') else json.loads(result['content'][0]['text'])
try:
    previous = root / (provider + '-first.json')
    if previous.exists():
        first = json.loads(previous.read_text())
        result = call('worker_collect', {'taskIds':[first['taskId']], 'detail':True})
        assert result['tasks'][0]['resultSha256'] == first['hash'], result
        legacy_result = call('worker_collect', {'taskIds':[first['taskId']], 'detail':True}, legacy=True)
        assert legacy_result['tasks'][0]['resultSha256'] == first['hash'], legacy_result
        other = 'codex' if provider == 'claude' else 'claude'
        foreign = json.loads((root / (other + '-first.json')).read_text())
        assert 'error' in call('worker_collect', {'taskIds':[foreign['taskId']]})
        save('reopened.json', result)
    else:
        assert 'error' in call('pass_off', {})
        models = call('worker_models', {})
        assert models['models'][0]['sharingAuthorized'] is True, models
        task = call('worker_start', {'schemaVersion':1,'requestId':'one','kind':'review','title':'Review total','brief':'Check this synthetic fixture','readFiles':['fixture.txt'],'model':'opencode/muse-spark-1.3-contributor-free'})
        assert 'taskId' in task, task
        retry = call('worker_start', {'schemaVersion':1,'requestId':'one','kind':'review','title':'Review total','brief':'Check this synthetic fixture','readFiles':['fixture.txt'],'model':'opencode/muse-spark-1.3-contributor-free'}, legacy=True)
        assert retry['taskId'] == task['taskId'], retry
        result = call('worker_collect', {'taskIds':[task['taskId']], 'waitMs':5000,'detail':True})
        assert result['tasks'][0]['state'] == 'completed', result
        assert call('notice', {})['taskIds'] == [task['taskId']]
        digest = result['tasks'][0]['resultSha256']
        review = call('worker_review', {'requestId':'review','taskId':task['taskId'],'resultSha256':digest,'disposition':'accepted','notes':'Verified 4 + 3 + 5 = 12.'})
        assert review['received'] is True, review
        assert call('notice', {})['taskIds'] == []
        save('first.json', {'taskId':task['taskId'],'hash':digest})
    while True: time.sleep(0.1)
except BaseException:
    save('failure.json', traceback.format_exc())
    raise
";
const REPORT: &str = r"#!/usr/bin/python3
import json, os, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
prompt = sys.stdin.read()
assert '4 + 3 + 5' in prompt
assert os.listdir('.') == []
with (root / 'calls.txt').open('a') as f: f.write('one\n')
report = {'schemaVersion':1,'outcome':'report','summary':'Total is 12','findings':[{'claim':'Total is 12','evidence':'fixture.txt:1: 4 + 3 + 5','uncertainty':'none'}],'missingContext':[]}
print(json.dumps({'type':'text','sessionID':'ses_fixture','part':{'text':json.dumps(report)}}))
print(json.dumps({'type':'step_finish','sessionID':'ses_fixture','part':{'reason':'stop'}}))
";
async fn report_client_file(root: &Path, provider: &str, phase: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(failure) = fs::read_to_string(root.join(format!("{provider}-failure.json"))) {
                panic!("{failure}");
            }
            if let Ok(bytes) = fs::read(root.join(format!("{provider}-{phase}.json")))
                && let Ok(value) = serde_json::from_slice(&bytes)
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("report client timed out: {provider}-{phase}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One fixture verifies both live owners and their resumed inboxes.
async fn native_report_workers_reconnect_both_providers_without_replay_or_cross_inbox_access() {
    let control = tempfile::tempdir_in("/private/tmp").unwrap();
    fs::write(control.path().join("client.py"), CLIENT).unwrap();
    let worker = control.path().join("report.py");
    fs::write(&worker, REPORT).unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700)).unwrap();
    let mut fixture = Fixture::new();
    fixture.dispatcher.close().await;
    let launches = TerminalLaunchCatalog::new(
        [
            TerminalLaunchProfileId::Claude,
            TerminalLaunchProfileId::Codex,
        ]
        .into_iter()
        .map(|profile| {
            let lead = control.path().join(format!("lead-{}", profile.as_str()));
            fs::write(
                &lead,
                format!(
                    "#!/bin/sh\nexec /usr/bin/python3 '{}' '{}' '{}'\n",
                    control.path().join("client.py").display(),
                    control.path().display(),
                    profile.as_str()
                ),
            )
            .unwrap();
            fs::set_permissions(&lead, fs::Permissions::from_mode(0o700)).unwrap();
            TerminalLaunchProgram::new(
                profile,
                lead,
                vec![format!("capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}").into()],
                vec![],
            )
            .unwrap()
            .with_provider_session_capture()
            .unwrap()
        }),
    )
    .unwrap()
    .with_pane_environment(vec![(
        OsString::from(PANE_MCP_SOCKET_ENV),
        pane_mcp_socket_path(control.path()).into_os_string(),
    )])
    .unwrap();
    let (sender, receiver) = mpsc::channel(64);
    fixture.pushes = receiver;
    fixture.dispatcher = Arc::new(
        TerminalHostDispatcher::with_backend_launches_and_claude_store(
            Arc::new(FakeRuntime::default()),
            fixture.desktop.clone(),
            fixture.releases.clone(),
            sender,
            Some(fixture.plane.as_ref().unwrap().outbound.clone()),
            TerminalServiceConfig::default(),
            Arc::new(SystemTerminalPtyBackend::default()),
            launches,
            None,
            Some(Arc::new(ReportSessions)),
        )
        .unwrap(),
    );
    fs::write(fixture.root.join("fixture.txt"), "4 + 3 + 5\n").unwrap();
    let server =
        PaneMcpServer::start_with_reports(control.path(), fixture.dispatcher.clone(), Some(worker))
            .unwrap();
    assert!(!control.path().join("panes/delegates").exists());
    interface_collaboration::workers::grants::write(
        &control.path().join("panes/report-workers"),
        interface_collaboration::workers::Policy {
            source: fixture.root.clone(),
            files: std::collections::BTreeSet::from(["fixture.txt".into()]),
            contributor_data_allowed: true,
        },
    )
    .unwrap();
    let mut panes = Vec::new();
    for provider in ["claude", "codex"] {
        let id = uuid::Uuid::new_v4();
        let mut create = create_payload(id, &fixture.root);
        create["launch"] = json!({"version":1,"profileId":provider});
        let pane = fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, create)
            .await
            .unwrap();
        report_client_file(control.path(), provider, "first").await;
        panes.push((provider, id, pane["generation"].clone()));
    }
    for (provider, id, generation) in panes {
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.close_terminal,
                json!({"id":id,"generation":generation}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let list = fixture
                    .dispatcher
                    .dispatch(CHANNELS.list_terminals, Value::Null)
                    .await
                    .unwrap();
                if list["terminals"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["id"] == json!(id) && p["state"] == "exited")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.restart_terminal,
                json!({"id":id,"generation":generation}),
            )
            .await
            .unwrap();
        report_client_file(control.path(), provider, "reopened").await;
    }
    assert_eq!(
        fs::read_to_string(control.path().join("calls.txt"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("fixture.txt")).unwrap(),
        "4 + 3 + 5\n"
    );
    fixture.shutdown().await;
    server.shutdown().await;
    assert!(!pane_mcp_socket_path(control.path()).exists());
    assert!(
        !crate::pane_mcp::cheap_agents_mcp_socket_path(&pane_mcp_socket_path(control.path()))
            .exists()
    );
}

#[tokio::test]
async fn an_empty_worker_notice_answers_before_identifying_the_caller() {
    use crate::mcp_transport::test_support::exchange;
    use tokio::io::AsyncBufReadExt as _;

    let fixture = Fixture::new();
    let server = PaneMcpServer::start_with_reports(
        fixture.temporary.path(),
        fixture.dispatcher.clone(),
        Some(PathBuf::from("/usr/bin/false")),
    )
    .expect("pane socket");
    let stream = tokio::net::UnixStream::connect(pane_mcp_socket_path(fixture.temporary.path()))
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(reader).lines();
    // This test process is nobody's pane, so identifying it as a caller would
    // fail. With no result waiting anywhere, the per-tool-call notice must not
    // try: it answers empty before any process-tree walk.
    let notice = exchange(
        &mut writer,
        &mut lines,
        json!({"jsonrpc": "2.0", "id": 1, "method": "interface/worker-notice"}),
    )
    .await;
    assert_ne!(notice["result"]["isError"], true, "{notice}");
    let text = notice["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        interface_collaboration::workers::empty_notice()
    );
    // Every other report tool still identifies the caller first.
    let refused = exchange(
        &mut writer,
        &mut lines,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "worker_models", "arguments": {}}
        }),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    server.stop();
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_result_for_another_conversation_leaves_the_notice_on_the_fast_path() {
    use crate::mcp_transport::test_support::exchange;
    use crate::report_workers::conversation_owner;
    use interface_collaboration::workers::{MODEL, Policy, grants};
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncBufReadExt as _;

    const BOUND: &str = "11111111-1111-4111-8111-111111111111";
    let launch_directory = TempDir::new().expect("launch directory");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable(&launch_directory, "claude"),
        vec![OsString::from(format!(
            "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .unwrap()
    .with_provider_session_capture()
    .unwrap()])
    .unwrap();
    let fixture =
        Fixture::new_with_launches_and_provider_sessions(catalog, Some(Arc::new(ReportSessions)));
    // A running Claude pane bound to one conversation.
    let wire_id = uuid::Uuid::new_v4();
    let mut create = create_payload(wire_id, &fixture.root);
    create["launch"] = json!({"version": 1, "profileId": "claude"});
    create["launchSession"] = json!({"version": 1, "id": BOUND, "resume": true});
    fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, create)
        .await
        .unwrap();
    assert_eq!(fixture.dispatcher.bound_provider_sessions(), [BOUND]);
    // Worker packets refuse a data root reached through a symbolic link, as
    // the default temporary directory is.
    let control = tempfile::tempdir_in("/private/tmp").unwrap();
    let server = PaneMcpServer::start_with_reports(
        control.path(),
        fixture.dispatcher.clone(),
        Some(PathBuf::from("/usr/bin/false")),
    )
    .expect("pane socket");
    let workers = server.reports().expect("report workers");
    let policy = Policy {
        source: fixture.root.clone(),
        files: std::collections::BTreeSet::new(),
        contributor_data_allowed: true,
    };
    grants::write(&control.path().join("panes/report-workers"), policy.clone()).unwrap();
    // Each task fails at once (the worker is /usr/bin/false) and stays
    // unreviewed, so it is unreceived for its owner.
    let finish = async |owner: &str, request: &str| {
        let task = workers
            .service
            .call_scoped(
                owner,
                &policy,
                "worker_start",
                json!({"schemaVersion":1,"requestId":request,"kind":"review","title":"Check",
                    "brief":"Check the fixture","readFiles":[],"model":MODEL}),
            )
            .await
            .unwrap();
        let task_id = task["taskId"].as_str().unwrap().to_owned();
        let collected = workers
            .service
            .collect(owner, &[task_id], 0, 10_000, false)
            .await
            .unwrap();
        assert_eq!(collected["tasks"][0]["state"], "failed", "{collected}");
    };
    let notice = async || {
        let stream = tokio::net::UnixStream::connect(pane_mcp_socket_path(control.path()))
            .await
            .expect("connect");
        let (reader, mut writer) = stream.into_split();
        let mut lines = tokio::io::BufReader::new(reader).lines();
        exchange(
            &mut writer,
            &mut lines,
            json!({"jsonrpc": "2.0", "id": 1, "method": "interface/worker-notice"}),
        )
        .await
    };
    finish(
        &conversation_owner("claude", "22222222-2222-4222-8222-222222222222"),
        "elsewhere",
    )
    .await;
    // No running pane is in that conversation: the notice answers empty
    // without identifying its caller.
    let answer = notice().await;
    assert_ne!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        interface_collaboration::workers::empty_notice()
    );
    assert_eq!(workers.caller_lookups.load(Ordering::SeqCst), 0);
    // A result for the pane's own conversation (as either provider) sends
    // notices through caller identification again. This test process is
    // nobody's pane, so that refuses.
    finish(&conversation_owner("codex", BOUND), "here").await;
    let answer = notice().await;
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    assert_eq!(workers.caller_lookups.load(Ordering::SeqCst), 1);
    server.stop();
    fixture.shutdown().await;
}
