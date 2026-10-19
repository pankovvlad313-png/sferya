//! Reconstruction using observed IPC/SQLite contracts and the public grammers 0.10 API.
//! This is new Rust source, not original decompiler output. See ../../../../integrations/RECOVERY.md.
use std::{collections::HashMap, fs, io, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use grammers_client::{Client, SenderPool, SignInError, client::{LoginToken, PasswordToken}, peer::Peer};
use grammers_session::{BoxFuture, Session, SessionData, types::{DcOption, PeerId, PeerInfo, PeerRef, UpdateState, UpdatesState}};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use rusqlite::{params, OptionalExtension};
use crate::storage::Store;

#[path="cloud.rs"] mod cloud;

pub struct IntegrationState {
    data_dir: PathBuf,
    client: Option<Client>,
    session: Option<Arc<PersistentSession>>,
    runner: Option<tokio::task::JoinHandle<()>>,
    login_token: Option<LoginToken>,
    password_token: Option<PasswordToken>,
    peers: HashMap<String, PeerRef>,
}

impl IntegrationState {
    pub fn new(data_dir: &Path) -> Self {
        Self { data_dir: data_dir.into(), client: None, session: None, runner: None,
            login_token: None, password_token: None, peers: HashMap::new() }
    }

    pub async fn invoke(&mut self, cmd: &str, args: Value, store: &Mutex<Store>) -> Result<Value, String> {
        match cmd {
            "telegram_status" => self.status(store).await,
            "telegram_start_login" => self.start_login(&args).await,
            "telegram_finish_login" => self.finish_login(&args).await,
            "telegram_finish_password" => self.finish_password(&args).await,
            "telegram_list_chats" => self.list_chats(store).await,
            "telegram_select_chat" => self.select_chat(&args, store).await,
            "telegram_sync" => self.sync_telegram(store).await,
            "telegram_disconnect" => self.disconnect(store).await,
            "cloud_status"|"cloud_connect"|"cloud_sync"|"cloud_disconnect" => cloud::invoke(&self.data_dir,cmd,&args,store).await,
            _ => Err(format!("Неизвестная команда интеграции: {cmd}")),
        }
    }

    fn session_path(&self) -> PathBuf { self.data_dir.join("telegram-account.recovered.dpapi") }

    async fn connect_session(&mut self, api_id: Option<i32>) -> Result<(), String> {
        if self.client.is_some() { return Ok(()); }
        let session=Arc::new(PersistentSession::open(self.session_path(), api_id).map_err(|e|format!("Не удалось прочитать восстановленную сессию Telegram: {e}"))?);
        let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), session.api_id);
        self.runner=Some(tokio::spawn(async move { runner.run().await; }));
        self.client=Some(Client::new(handle));
        self.session=Some(session);
        Ok(())
    }

    async fn authorized(&mut self) -> Result<bool,String> {
        if self.client.is_none() {
            if !self.session_path().is_file() { return Ok(false); }
            self.connect_session(None).await?;
        }
        self.client.as_ref().unwrap().is_authorized().await.map_err(|e|format!("Не удалось проверить вход в Telegram: {e}"))
    }

    async fn status(&mut self, store:&Mutex<Store>) -> Result<Value,String> {
        let connected=self.authorized().await?;
        let pending={let db=store.lock().map_err(|_|"Хранилище недоступно")?;
            db.conn.query_row("SELECT count(*) FROM telegram_candidates WHERE status='pending'",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?};
        let app_configured=env_credentials().is_some();
        Ok(json!({"connected":connected,"app_configured":app_configured,"pending":pending}))
    }

    async fn start_login(&mut self,args:&Value)->Result<Value,String> {
        if self.authorized().await? { return Err("Сначала отключите текущее соединение".into()); }
        let given_id=args.get("apiId").and_then(Value::as_i64).and_then(|n|i32::try_from(n).ok());
        let given_hash=args.get("apiHash").and_then(Value::as_str).filter(|s|!s.trim().is_empty());
        let (api_id,api_hash)=match (given_id,given_hash) {
            (Some(id),Some(hash)) => (id,hash.trim().to_owned()),
            (None,None) => env_credentials().ok_or("Введите свои API ID и API hash с my.telegram.org/apps")?,
            _ => return Err("Введите API ID и API hash вместе".into()),
        };
        if api_id<=0 || api_hash.len()!=32 || !api_hash.bytes().all(|b|b.is_ascii_hexdigit()) { return Err("Некорректные API ID или API hash Telegram".into()); }
        let phone=string_arg(args,"phone")?.trim();
        if !phone.starts_with('+')||phone.len()<8||phone.len()>20||!phone[1..].chars().all(|c|c.is_ascii_digit()||c==' '||c=='-') { return Err("Введите телефон в международном формате, например +79990000000".into()); }
        self.close_connection().await;
        // Start a separate recovered session. Hash, phone, one-time codes and password are not persisted.
        remove_if_present(&self.session_path())?;
        self.connect_session(Some(api_id)).await?;
        let token=self.client.as_ref().unwrap().request_login_code(phone,&api_hash).await.map_err(|e|format!("Telegram не отправил код: {e}"))?;
        self.login_token=Some(token);
        Ok(Value::Null)
    }

    async fn finish_login(&mut self,args:&Value)->Result<Value,String> {
        let code=string_arg(args,"code")?.trim().replace(' ',"");
        let token=self.login_token.as_ref().ok_or("Сначала запросите код Telegram")?;
        let client=self.client.as_ref().ok_or("Соединение Telegram не установлено")?;
        match client.sign_in(token,&code).await {
            Ok(_) => { self.login_token=None; self.flush()?; Ok(json!("connected")) },
            Err(SignInError::PasswordRequired(token)) => {self.password_token=Some(token);self.login_token=None;Ok(json!("password"))},
            Err(e) => Err(format!("Не удалось войти в Telegram: {e}")),
        }
    }

    async fn finish_password(&mut self,args:&Value)->Result<Value,String> {
        let password=string_arg(args,"password")?;
        let token=self.password_token.take().ok_or("Telegram не запрашивал пароль")?;
        let client=self.client.as_ref().ok_or("Соединение Telegram не установлено")?;
        match client.check_password(token,password).await {
            Ok(_) => { self.flush()?; Ok(Value::Null) },
            Err(SignInError::InvalidPassword(token)) => { self.password_token=Some(token);Err("Неверный пароль Telegram".into()) },
            Err(e) => Err(format!("Не удалось войти в Telegram: {e}")),
        }
    }

    fn flush(&self)->Result<(),String> {self.session.as_ref().ok_or("Сессия Telegram не найдена")?.persist().map_err(|e|format!("Не удалось сохранить вход в Telegram: {e}"))}

    async fn list_chats(&mut self,store:&Mutex<Store>)->Result<Value,String> {
        if !self.authorized().await? {return Err("Сначала подключите Telegram".into());}
        let enabled={let db=store.lock().map_err(|_|"Хранилище недоступно")?;
            let mut q=db.conn.prepare("SELECT chat_id,enabled FROM telegram_chats").map_err(|e|e.to_string())?;
            let result=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?!=0))).map_err(|e|e.to_string())?.collect::<Result<HashMap<_,_>,_>>().map_err(|e|e.to_string())?;result};
        let client=self.client.as_ref().unwrap();
        let mut dialogs=client.iter_dialogs(); let mut chats=Vec::new();self.peers.clear();
        while let Some(dialog)=dialogs.next().await.map_err(|e|format!("Не удалось получить чаты Telegram: {e}"))? {
            let peer=dialog.peer();let Some(id)=peer.id().bot_api_dialog_id().map(|i|i.to_string()) else {continue};
            let kind=match peer {Peer::User(_)=>"Личный чат",Peer::Group(_)=>"Группа",Peer::Channel(_)=>"Канал"};
            let name=peer.name().unwrap_or("Без названия");
            self.peers.insert(id.clone(),dialog.peer_ref());
            chats.push(json!({"id":id,"name":name,"kind":kind,"selected":enabled.get(&id).copied().unwrap_or(false)}));
        }
        chats.sort_by(|a,b|a["name"].as_str().cmp(&b["name"].as_str()));
        self.flush()?;Ok(json!(chats))
    }

    async fn select_chat(&mut self,args:&Value,store:&Mutex<Store>)->Result<Value,String> {
        let id=string_arg(args,"chatId")?.to_owned();
        let selected=args.get("selected").and_then(Value::as_bool).ok_or("Не задано selected")?;
        if !selected {let db=store.lock().map_err(|_|"Хранилище недоступно")?;db.conn.execute("UPDATE telegram_chats SET enabled=0 WHERE chat_id=?1",[&id]).map_err(|e|e.to_string())?;return Ok(Value::Null);}
        let chats=self.list_chats(store).await?;
        let chat=chats.as_array().unwrap().iter().find(|c|c["id"]==id).ok_or("Чат Telegram не найден")?;
        let peer=*self.peers.get(&id).ok_or("Чат Telegram недоступен")?;
        // Selection establishes an exclusive high-water mark. Older conversation history is not imported.
        let mut latest=self.client.as_ref().unwrap().iter_messages(peer).limit(1);
        let last=latest.next().await.map_err(|e|e.to_string())?.map(|m|m.id()).unwrap_or(0);
        let db=store.lock().map_err(|_|"Хранилище недоступно")?;
        db.conn.execute("INSERT INTO telegram_chats(chat_id,name,enabled,last_message_id) VALUES(?1,?2,1,?3) ON CONFLICT(chat_id) DO UPDATE SET name=excluded.name,enabled=1,last_message_id=excluded.last_message_id",params![id,chat["name"].as_str().unwrap_or("Без названия"),last]).map_err(|e|e.to_string())?;
        Ok(Value::Null)
    }

    async fn sync_telegram(&mut self,store:&Mutex<Store>)->Result<Value,String> {
        if !self.authorized().await? {return Err("Сначала подключите Telegram".into());}
        if self.peers.is_empty(){self.list_chats(store).await?;}
        let selected={let db=store.lock().map_err(|_|"Хранилище недоступно")?;
            let mut q=db.conn.prepare("SELECT chat_id,name,last_message_id FROM telegram_chats WHERE enabled=1").map_err(|e|e.to_string())?;
            let result=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i32>(2)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;result};
        for (id,name,last) in selected {
            let peer=*self.peers.get(&id).ok_or_else(||format!("Выбранный чат {name} больше недоступен"))?;
            let mut messages=self.client.as_ref().unwrap().iter_messages(peer).reverse(true).offset_id(last).limit(500);
            let mut next=last;let mut candidates=Vec::new();
            while let Some(message)=messages.next().await.map_err(|e|format!("Не удалось прочитать выбранный чат {name}: {e}"))? {
                if message.id()<=last {continue;} next=next.max(message.id());
                if let Some((confidence,bucket))=classify_message(message.text()) {
                    let sender=message.sender().and_then(Peer::name).unwrap_or("Telegram").to_owned();
                    candidates.push((message.id(),sender,message.text().to_owned(),message.date().to_rfc3339(),confidence,bucket));
                }
            }
            // DB lock begins only after all requests for this batch have completed.
            let mut db=store.lock().map_err(|_|"Хранилище недоступно")?;
            let tx=db.conn.transaction().map_err(|e|e.to_string())?;
            let still_enabled=tx.query_row("SELECT enabled FROM telegram_chats WHERE chat_id=?1",[&id],|r|r.get::<_,i64>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(0)!=0;
            if !still_enabled {continue;}
            for (message_id,sender,body,sent_at,confidence,bucket) in candidates {
                tx.execute("INSERT OR IGNORE INTO telegram_candidates(chat_id,message_id,chat_name,sender,body,sent_at,confidence,bucket) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,message_id,name,sender,body,sent_at,confidence,bucket]).map_err(|e|e.to_string())?;
            }
            tx.execute("UPDATE telegram_chats SET last_message_id=MAX(last_message_id,?1) WHERE chat_id=?2",params![next,id]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e|e.to_string())?;
        }
        self.flush()?;Ok(Value::Null)
    }

    async fn disconnect(&mut self,store:&Mutex<Store>)->Result<Value,String> {
        // Reconstructed connection may be revoked remotely; errors are reported before destroying its local key.
        if let Some(client)=self.client.as_ref() { if client.is_authorized().await.map_err(|e|e.to_string())? {client.sign_out().await.map_err(|e|format!("Не удалось завершить сессию Telegram: {e}"))?;} }
        self.close_connection().await;remove_if_present(&self.session_path())?;
        let db=store.lock().map_err(|_|"Хранилище недоступно")?;
        db.conn.execute_batch("BEGIN;DELETE FROM telegram_candidates;DELETE FROM telegram_chats;COMMIT;").map_err(|e|e.to_string())?;
        Ok(Value::Null)
    }

    async fn close_connection(&mut self){
        if let Some(client)=self.client.take(){client.disconnect();}
        if let Some(runner)=self.runner.take(){runner.abort();let _=runner.await;}
        self.session=None;self.login_token=None;self.password_token=None;self.peers.clear();
    }
    fn drop_connection(&mut self){if let Some(client)=self.client.take(){client.disconnect();}if let Some(runner)=self.runner.take(){runner.abort();}self.session=None;self.login_token=None;self.password_token=None;self.peers.clear();}
}

impl Drop for IntegrationState {fn drop(&mut self){self.drop_connection();}}

fn string_arg<'a>(args:&'a Value,key:&str)->Result<&'a str,String>{args.get(key).and_then(Value::as_str).ok_or_else(||format!("Не задано поле {key}"))}
mod original_client { include!(concat!(env!("OUT_DIR"),"/original_client.rs")); }
fn env_credentials()->Option<(i32,String)>{
    if let (Ok(id),Ok(hash))=(std::env::var("SFERY_TELEGRAM_API_ID"),std::env::var("SFERY_TELEGRAM_API_HASH")) {
        if let Ok(id)=id.parse::<i32>() {if id>0&&hash.len()==32&&hash.bytes().all(|b|b.is_ascii_hexdigit()){return Some((id,hash));}}
    }
    original_client::ORIGINAL_API_HASH.map(|hash|(original_client::ORIGINAL_API_ID,hash.to_owned()))
}
fn remove_if_present(path:&Path)->Result<(),String>{match fs::remove_file(path){Ok(())=>Ok(()),Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(e.to_string())}}

/// New conservative local classifier. Exact original rules/weights remain unrecovered.
fn classify_message(text:&str)->Option<(f64,&'static str)>{
    let t=text.trim().to_lowercase();if t.is_empty(){return None;}
    let direct=["сделай","сделать","подготовь","подготовить","отправь","отправить","проверь","проверить","пришли","пришлите","напомни","купить","позвони","задача:","todo:"];
    let uncertain=["нужно","надо","пожалуйста","можешь","можете","не забуд","дедлайн","до пятницы","до понедельника","на завтра","на сегодня"];
    if direct.iter().any(|s|t.contains(s)){Some((0.85,"high"))}else if uncertain.iter().any(|s|t.contains(s)){Some((0.5,"maybe"))}else{None}
}

#[derive(Serialize,Deserialize)]
struct StoredSession {api_id:i32,home_dc:i32,dc_options:Vec<DcOption>,peer_infos:Vec<PeerInfo>,updates_state:UpdatesState}

struct PersistentSession {api_id:i32,path:PathBuf,data:Mutex<SessionData>}
impl PersistentSession {
    fn open(path:PathBuf,api_id:Option<i32>)->io::Result<Self>{
        let (api_id,data)=if path.is_file(){let raw=unprotect(&fs::read(&path)?)?;let s:StoredSession=serde_json::from_slice(&raw).map_err(io::Error::other)?;
            (s.api_id,SessionData{home_dc:s.home_dc,dc_options:s.dc_options.into_iter().map(|d|(d.id,d)).collect(),peer_infos:s.peer_infos.into_iter().map(|p|(p.id(),p)).collect(),updates_state:s.updates_state})
        }else{(api_id.ok_or_else(||io::Error::other("API ID не задан"))?,SessionData::default())};
        if api_id<=0{return Err(io::Error::other("Некорректный API ID"));}
        Ok(Self{api_id,path,data:Mutex::new(data)})
    }
    fn data(&self)->io::Result<std::sync::MutexGuard<'_,SessionData>>{self.data.lock().map_err(|_|io::Error::other("Сессия Telegram недоступна"))}
    fn persist(&self)->io::Result<()>{let data=self.data()?;self.persist_data(&data) }
    fn persist_data(&self,data:&SessionData)->io::Result<()>{
        let s=StoredSession{api_id:self.api_id,home_dc:data.home_dc,dc_options:data.dc_options.values().cloned().collect(),peer_infos:data.peer_infos.values().cloned().collect(),updates_state:data.updates_state.clone()};
        let encrypted=protect(&serde_json::to_vec(&s).map_err(io::Error::other)?)?;
        if let Some(parent)=self.path.parent(){fs::create_dir_all(parent)?;}
        let tmp=self.path.with_extension("dpapi.tmp");fs::write(&tmp,encrypted)?;fs::rename(tmp,&self.path)
    }
    fn update(&self,operation:impl FnOnce(&mut SessionData))->io::Result<()>{let mut data=self.data()?;operation(&mut data);self.persist_data(&data)}
}
impl Session for PersistentSession {
    type Error=io::Error;
    fn home_dc_id(&self)->io::Result<i32>{Ok(self.data()?.home_dc)}
    fn set_home_dc_id(&self,id:i32)->BoxFuture<'_,io::Result<()>>{Box::pin(async move{self.update(|d|d.home_dc=id)})}
    fn dc_option(&self,id:i32)->io::Result<Option<DcOption>>{Ok(self.data()?.dc_options.get(&id).cloned())}
    fn set_dc_option(&self,dc:&DcOption)->BoxFuture<'_,io::Result<()>>{let dc=dc.clone();Box::pin(async move{self.update(|d|{d.dc_options.insert(dc.id,dc);})})}
    fn peer(&self,id:PeerId)->BoxFuture<'_,io::Result<Option<PeerInfo>>>{Box::pin(async move{Ok(self.data()?.peer_infos.get(&id).cloned())})}
    fn cache_peer(&self,peer:&PeerInfo)->BoxFuture<'_,io::Result<()>>{let peer=peer.clone();Box::pin(async move{self.update(|d|{d.peer_infos.entry(peer.id()).and_modify(|old|{old.extend_info(&peer);}).or_insert(peer);})})}
    fn updates_state(&self)->BoxFuture<'_,io::Result<UpdatesState>>{Box::pin(async move{Ok(self.data()?.updates_state.clone())})}
    fn set_update_state(&self,state:UpdateState)->BoxFuture<'_,io::Result<()>>{Box::pin(async move{self.update(|d|match state{
        UpdateState::All(s)=>d.updates_state=s,
        UpdateState::Primary{pts,date,seq}=>{d.updates_state.pts=pts;d.updates_state.date=date;d.updates_state.seq=seq;},
        UpdateState::Secondary{qts}=>d.updates_state.qts=qts,
        UpdateState::Channel{id,pts}=>{d.updates_state.channels.retain(|c|c.id!=id);d.updates_state.channels.push(grammers_session::types::ChannelState{id,pts});}
    })})}
}

#[cfg(windows)]
fn crypt(bytes:&[u8],encrypt:bool)->io::Result<Vec<u8>>{
    use std::ffi::c_void;
    #[repr(C)]struct Blob{len:u32,ptr:*mut u8}
    #[link(name="crypt32")]unsafe extern "system"{
        fn CryptProtectData(input:*const Blob,description:*const u16,entropy:*const Blob,reserved:*mut c_void,prompt:*mut c_void,flags:u32,output:*mut Blob)->i32;
        fn CryptUnprotectData(input:*const Blob,description:*mut *mut u16,entropy:*const Blob,reserved:*mut c_void,prompt:*mut c_void,flags:u32,output:*mut Blob)->i32;
    }
    #[link(name="kernel32")]unsafe extern "system"{fn LocalFree(ptr:*mut c_void)->*mut c_void;}
    let input=Blob{len:u32::try_from(bytes.len()).map_err(io::Error::other)?,ptr:bytes.as_ptr() as *mut u8};let mut output=Blob{len:0,ptr:std::ptr::null_mut()};
    let success=unsafe{if encrypt{CryptProtectData(&input,std::ptr::null(),std::ptr::null(),std::ptr::null_mut(),std::ptr::null_mut(),1,&mut output)}else{CryptUnprotectData(&input,std::ptr::null_mut(),std::ptr::null(),std::ptr::null_mut(),std::ptr::null_mut(),1,&mut output)}};
    if success==0{return Err(io::Error::last_os_error());}
    let result=unsafe{std::slice::from_raw_parts(output.ptr,output.len as usize).to_vec()};unsafe{LocalFree(output.ptr.cast());}Ok(result)
}
#[cfg(windows)]fn protect(bytes:&[u8])->io::Result<Vec<u8>>{crypt(bytes,true)}
#[cfg(windows)]fn unprotect(bytes:&[u8])->io::Result<Vec<u8>>{crypt(bytes,false)}
#[cfg(not(windows))]fn protect(_: &[u8])->io::Result<Vec<u8>>{Err(io::Error::other("DPAPI session storage requires Windows"))}
#[cfg(not(windows))]fn unprotect(_: &[u8])->io::Result<Vec<u8>>{Err(io::Error::other("DPAPI session storage requires Windows"))}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn classifier_distinguishes_requests(){assert_eq!(classify_message("Подготовь отчёт"),Some((0.85,"high")));assert_eq!(classify_message("Можешь посмотреть завтра?"),Some((0.5,"maybe")));assert_eq!(classify_message("Доброе утро"),None);}
    #[cfg(windows)]#[test]fn windows_dpapi_roundtrip(){let encrypted=protect(b"recovery-test-only").unwrap();assert_ne!(encrypted,b"recovery-test-only");assert_eq!(unprotect(&encrypted).unwrap(),b"recovery-test-only");}
}
