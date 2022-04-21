use async_std::sync::{Arc, Mutex};
use std::{fs::File, io, io::Read, path::PathBuf};

use easy_parallel::Parallel;
use fxhash::{FxHashMap, FxHashSet};
use log::{debug, info};
use rand::{thread_rng, Rng};
use serde_json::{json, Value};
use simplelog::*;
use smol::Executor;

use termion::{async_stdin, event::Key, input::TermRead, raw::IntoRawMode};
use tui::{
    backend::{Backend, TermionBackend},
    Terminal,
};
use url::Url;

use darkfi::{
    error::{Error, Result},
    rpc::{jsonrpc, jsonrpc::JsonResult},
    util::{
        async_util,
        cli::{log_config, spawn_config, Config},
        join_config_path,
    },
};

use dnetview::{
    config::{DnvConfig, CONFIG_FILE_CONTENTS},
    model::{ConnectInfo, Model, NodeInfo, SelectableObject, SessionInfo},
    options::ProgramOptions,
    ui,
    view::{IdListView, InfoListView, View},
};

struct DNetView {
    url: Url,
    name: String,
}

impl DNetView {
    pub fn new(url: Url, name: String) -> Self {
        Self { url, name }
    }

    async fn request(&self, r: jsonrpc::JsonRequest) -> Result<Value> {
        let reply: JsonResult = match jsonrpc::send_request(&self.url, json!(r), None).await {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        match reply {
            JsonResult::Resp(r) => {
                debug!(target: "RPC", "<-- {}", serde_json::to_string(&r)?);
                Ok(r.result)
            }

            JsonResult::Err(e) => {
                debug!(target: "RPC", "<-- {}", serde_json::to_string(&e)?);
                Err(Error::JsonRpcError(e.error.message.to_string()))
            }

            JsonResult::Notif(n) => {
                debug!(target: "RPC", "<-- {}", serde_json::to_string(&n)?);
                Err(Error::JsonRpcError("Unexpected reply".to_string()))
            }
        }
    }

    // --> {"jsonrpc": "2.0", "method": "ping", "params": [], "id": 42}
    // <-- {"jsonrpc": "2.0", "result": "pong", "id": 42}
    async fn _ping(&self) -> Result<Value> {
        let req = jsonrpc::request(json!("ping"), json!([]));
        Ok(self.request(req).await?)
    }

    //--> {"jsonrpc": "2.0", "method": "poll", "params": [], "id": 42}
    // <-- {"jsonrpc": "2.0", "result": {"nodeID": [], "nodeinfo" [], "id": 42}
    async fn get_info(&self) -> Result<Value> {
        let req = jsonrpc::request(json!("get_info"), json!([]));
        Ok(self.request(req).await?)
    }
}

#[async_std::main]
async fn main() -> Result<()> {
    let options = ProgramOptions::load()?;

    let verbosity_level = options.app.occurrences_of("verbose");

    let (lvl, cfg) = log_config(verbosity_level)?;

    let file = File::create(&*options.log_path).unwrap();
    WriteLogger::init(lvl, cfg, file)?;
    info!("Log level: {}", lvl);

    let config_path = join_config_path(&PathBuf::from("dnetview_config.toml"))?;

    spawn_config(&config_path, CONFIG_FILE_CONTENTS)?;

    let config = Config::<DnvConfig>::load(config_path)?;

    let stdout = io::stdout().into_raw_mode()?;
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    terminal.clear()?;

    let ids = Mutex::new(FxHashSet::default());
    let infos = Mutex::new(FxHashMap::default());

    let model = Arc::new(Model::new(ids, infos));

    let nthreads = num_cpus::get();
    let (signal, shutdown) = async_channel::unbounded::<()>();

    let ex = Arc::new(Executor::new());
    let ex2 = ex.clone();

    let (_, result) = Parallel::new()
        .each(0..nthreads, |_| smol::future::block_on(ex.run(shutdown.recv())))
        .finish(|| {
            smol::future::block_on(async move {
                run_rpc(&config, ex2.clone(), model.clone()).await?;
                render(&mut terminal, model.clone()).await?;
                drop(signal);
                Ok::<(), darkfi::Error>(())
            })
        });

    result
}

async fn run_rpc(config: &DnvConfig, ex: Arc<Executor<'_>>, model: Arc<Model>) -> Result<()> {
    for node in config.nodes.clone() {
        let client = DNetView::new(Url::parse(&node.rpc_url)?, node.name);
        ex.spawn(poll(client, model.clone())).detach();
    }
    Ok(())
}

async fn poll(client: DNetView, model: Arc<Model>) -> Result<()> {
    loop {
        let reply = client.get_info().await?;

        if reply.as_object().is_some() && !reply.as_object().unwrap().is_empty() {
            parse_data(reply.as_object().unwrap(), &client, model.clone()).await?;
        } else {
            // TODO: error handling
            //debug!("Reply is empty");
        }
        async_util::sleep(2).await;
    }
}

// TODO: split into parse maunal/ inbound/ outbound functions
// make if/else into switch statements for clarity
async fn parse_data(
    reply: &serde_json::Map<String, Value>,
    client: &DNetView,
    model: Arc<Model>,
) -> io::Result<()> {
    // TODO
    let _ext_addr = reply.get("external_addr");

    let inbound_obj = &reply["session_inbound"];
    // TODO
    let manual_obj = &reply["session_manual"];
    let outbound_obj = &reply["session_outbound"];

    let mut model_vec: Vec<SelectableObject> = Vec::new();
    let connections: Vec<ConnectInfo> = Vec::new();
    let sessions: Vec<SessionInfo> = Vec::new();

    let node_id = generate_id();
    let node_name = &client.name;

    parse_inbound(inbound_obj, connections.clone(), sessions.clone(), model_vec.clone(), node_id);
    parse_outbound(outbound_obj, connections.clone(), sessions.clone(), model_vec.clone(), node_id);
    parse_manual(manual_obj, connections.clone(), sessions.clone(), model_vec.clone(), node_id);

    let node_info = NodeInfo::new(node_id, node_name.to_string(), sessions);
    let node = SelectableObject::Node(node_info.clone());
    model_vec.push(node);

    // TODO: write data to HashMaps and HashSets
    Ok(())
}

fn parse_inbound(
    inbound_obj: &Value,
    mut connections: Vec<ConnectInfo>,
    mut sessions: Vec<SessionInfo>,
    mut model_vec: Vec<SelectableObject>,
    node_id: u32,
) {
    let i_connected = &inbound_obj["connected"];
    let i_session_id = generate_id();
    if i_connected.as_object().unwrap().is_empty() {
        // channel is empty. initialize with empty values
        let i_connect_id = generate_id();
        let addr = "Null".to_string();
        let msg = "Null".to_string();
        let status = "Null".to_string();
        let is_empty = true;
        let parent = i_session_id;
        // TODO
        let state = "Null".to_string();
        // TODO
        let msg_log = Vec::new();
        let connect_info =
            ConnectInfo::new(i_connect_id, addr, is_empty, msg, status, state, msg_log, parent);
        connections.push(connect_info.clone());
        let connect = SelectableObject::Connect(connect_info.clone());
        model_vec.push(connect);
    } else {
        // channel is not empty. initialize with whole values
        let i_connect_id = generate_id();
        let ic = i_connected.as_object().unwrap();
        for k in ic.keys() {
            let node = ic.get(k);
            let addr = k.to_string();
            let msg = node.unwrap().get("last_msg").unwrap().as_str().unwrap().to_string();
            let status = node.unwrap().get("last_status").unwrap().as_str().unwrap().to_string();
            let state = node.unwrap().get("state").unwrap().as_str().unwrap().to_string();
            let is_empty = false;
            let parent = i_session_id;
            // TODO
            let msg_log = Vec::new();
            let connect_info =
                ConnectInfo::new(i_connect_id, addr, is_empty, msg, status, state, msg_log, parent);
            connections.push(connect_info.clone());
            let connect = SelectableObject::Connect(connect_info.clone());
            model_vec.push(connect);
        }
    }
    let i_session_info = SessionInfo::new(i_session_id, node_id, connections.clone());
    sessions.push(i_session_info.clone());
    let session = SelectableObject::Session(i_session_info.clone());
    model_vec.push(session);
}

fn parse_manual(
    manual_obj: &Value,
    mut connections: Vec<ConnectInfo>,
    mut sessions: Vec<SessionInfo>,
    mut model_vec: Vec<SelectableObject>,
    node_id: u32,
) {
    let m_session_id = generate_id();
    let m_connect_id = generate_id();
    let addr = "Null".to_string();
    let msg = "Null".to_string();
    let status = "Null".to_string();
    let is_empty = true;
    let parent = m_session_id;
    // TODO
    let state = "Null".to_string();
    // TODO
    let msg_log = Vec::new();
    let m_connect_info =
        ConnectInfo::new(m_connect_id, addr, is_empty, msg, status, state, msg_log, parent);
    connections.push(m_connect_info.clone());
    let connect = SelectableObject::Connect(m_connect_info.clone());
    model_vec.push(connect);
}

fn parse_outbound(
    outbound_obj: &Value,
    mut connections: Vec<ConnectInfo>,
    mut sessions: Vec<SessionInfo>,
    mut model_vec: Vec<SelectableObject>,
    node_id: u32,
) {
    // parse outbound connection data
    let outbound_slots = &outbound_obj["slots"];
    let o_session_id = generate_id();
    for slot in outbound_slots.as_array().unwrap() {
        let o_connect_id = generate_id();
        if slot["channel"].is_null() {
            // channel is empty. initialize with empty values
            let is_empty = true;
            let addr = "Null".to_string();
            let state = &slot["state"];
            let msg = "Null".to_string();
            let status = "Null".to_string();
            // placeholder for now
            let msg_log = Vec::new();
            let parent = o_session_id;
            let connect_info = ConnectInfo::new(
                o_connect_id,
                addr,
                is_empty,
                msg,
                status,
                state.as_str().unwrap().to_string(),
                msg_log,
                parent,
            );
            connections.push(connect_info.clone());
            let connect = SelectableObject::Connect(connect_info.clone());
            model_vec.push(connect);
        } else {
            // TODO: cleanup/ make style consistent
            // channel is not empty. initialize with whole values
            let is_empty = false;
            let addr = &slot["addr"];
            let state = &slot["state"];
            let msg = &slot["last_msg"];
            let status = &slot["last_status"];
            let parent = o_session_id;
            // TODO
            let msg_log = Vec::new();
            let connect_info = ConnectInfo::new(
                o_connect_id,
                addr.as_str().unwrap().to_string(),
                is_empty,
                msg.as_str().unwrap().to_string(),
                status.as_str().unwrap().to_string(),
                state.as_str().unwrap().to_string(),
                msg_log,
                parent,
            );
            connections.push(connect_info.clone());
            let connect = SelectableObject::Connect(connect_info.clone());
            model_vec.push(connect);
        }
    }
    let o_session_info = SessionInfo::new(o_session_id, node_id, connections.clone());
    sessions.push(o_session_info.clone());
    let session = SelectableObject::Session(o_session_info.clone());
    model_vec.push(session);
}

fn generate_id() -> u32 {
    let mut rng = thread_rng();
    let id: u32 = rng.gen();
    id
}
//fn is_empty_outbound(slots: Vec<Slot>) -> bool {
//    return slots.iter().all(|slot| slot.is_empty);
//}

async fn render<B: Backend>(terminal: &mut Terminal<B>, model: Arc<Model>) -> io::Result<()> {
    let mut asi = async_stdin();

    terminal.clear()?;

    let id_list = IdListView::new(FxHashSet::default());
    let info_list = InfoListView::new(FxHashMap::default());
    let mut view = View::new(id_list.clone(), info_list.clone());

    view.id_list.state.select(Some(0));
    view.info_list.index = 0;

    loop {
        //view.update(model.info_list.infos.lock().await.clone());
        terminal.draw(|f| {
            ui::ui(f, view.clone());
        })?;
        for k in asi.by_ref().keys() {
            match k.unwrap() {
                Key::Char('q') => {
                    terminal.clear()?;
                    return Ok(())
                }
                Key::Char('j') => {
                    view.id_list.next();
                }
                Key::Char('k') => {
                    view.id_list.previous();
                }
                _ => (),
            }
        }
    }
}
