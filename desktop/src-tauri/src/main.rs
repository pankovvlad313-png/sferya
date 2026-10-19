#![cfg_attr(not(test), windows_subsystem = "windows")]
mod attachments;
mod agreement;
mod compat;
mod integrations;
mod models;
mod storage;
mod upgrade;
use std::sync::{Arc,Mutex};
use serde_json::Value;
use tauri::{Manager,ipc::InvokeBody};
use storage::Store;

struct AppState {
    store: Mutex<Store>,
    uploads: Mutex<attachments::Attachments>,
    integrations: tokio::sync::Mutex<integrations::IntegrationState>,
}
fn lock<T>(value:&Mutex<T>)->Result<std::sync::MutexGuard<'_,T>,String> { value.lock().map_err(|_|"Ошибка блокировки состояния".into()) }
fn str_arg<'a>(v:&'a Value,key:&str)->Result<&'a str,String> { v.get(key).and_then(Value::as_str).ok_or_else(||format!("Отсутствует {key}")) }

async fn dispatch(state:Arc<AppState>, app:tauri::AppHandle, command:String, body:InvokeBody, headers:tauri::http::HeaderMap)->Result<Value,String> {
    if command=="append_attachment" {
        let id=headers.get("x-upload-id").and_then(|v|v.to_str().ok()).ok_or("Нет файла")?;
        let offset=headers.get("x-upload-offset").and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<u64>().ok()).ok_or("Нет позиции")?;
        let InvokeBody::Raw(bytes)=body else { return Err("Неверный формат файла".into()); };
        return lock(&state.uploads)?.append(id,offset,&bytes);
    }
    let args=match body { InvokeBody::Json(v)=>v, InvokeBody::Raw(_)=>return Err("Ожидалось JSON-тело".into()) };
    let args=compat::arguments(&command,args)?;
    if command.starts_with("cloud_") || matches!(command.as_str(),"telegram_status"|"telegram_start_login"|"telegram_finish_login"|"telegram_finish_password"|"telegram_disconnect"|"telegram_list_chats"|"telegram_select_chat"|"telegram_sync") {
        return state.integrations.lock().await.invoke(&command,args,&state.store).await;
    }
    match command.as_str() {
        "begin_attachment"=>lock(&state.uploads)?.begin(&*lock(&state.store)?,&args),
        "finish_attachment"=>lock(&state.uploads)?.finish(&mut *lock(&state.store)?,str_arg(&args,"id")?),
        "discard_attachments"=>lock(&state.uploads)?.discard(&mut *lock(&state.store)?,&args),
        "read_text_attachment"=>attachments::read_text(&*lock(&state.store)?,str_arg(&args,"id")?),
        "open_attachment"|"show_attachment"=> {
            let path=attachments::asset_path(&*lock(&state.store)?,str_arg(&args,"id")?)?;
            if command=="show_attachment" {
                std::process::Command::new("explorer.exe").arg(format!("/select,{}",path.display())).spawn().map_err(|e|e.to_string())?;
            } else { open::that(&path).map_err(|e|e.to_string())?; }
            Ok(Value::Null)
        },
        "open_url"=> {
            let url=str_arg(&args,"url")?;
            if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) { return Err("Неподдерживаемая схема ссылки".into()); }
            open::that(url).map_err(|e|e.to_string())?; Ok(Value::Null)
        },
        "agreement_status"=> agreement::status(&*lock(&state.store)?),
        "accept_agreement"=> agreement::accept(&*lock(&state.store)?, &args),
        "exit_app"=>{ app.exit(0); Ok(Value::Null) },
        _=>lock(&state.store)?.invoke(&command,args),
    }
}
fn main() {
    let builder=tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window)=app.get_webview_window("main") { let _=window.unminimize(); let _=window.show(); let _=window.set_focus(); }
        }))
        .plugin(tauri_plugin_process::init());
    #[cfg(feature="production-upgrade")]
    let builder=builder.plugin(tauri_plugin_updater::Builder::new().build());
    let result=builder
        .setup(|app| {
            let production=cfg!(feature="production-upgrade");
            if production && app.config().identifier!="local.sfery.planner" && app.config().identifier!="local.sfery.planner.upgrade-test" { return Err(std::io::Error::other("Для сборки обновления требуется tauri.production.conf.json").into()); }
            if production { upgrade::check_original_process().map_err(std::io::Error::other)?; }
            let default_dir=if production { app.path().app_data_dir()? } else { app.path().app_local_data_dir()? };
            let data_dir=std::env::var_os("SFERY_RECOVERED_DATA_DIR").map(std::path::PathBuf::from).unwrap_or(default_dir);
            let upgrade=upgrade::Upgrade::prepare(&data_dir,&app.package_info().version.to_string()).map_err(std::io::Error::other)?;
            let store=Store::new(&data_dir).map_err(std::io::Error::other)?;
            upgrade.finish().map_err(std::io::Error::other)?;
            app.manage(upgrade);
            app.asset_protocol_scope().allow_directory(store.attachments_dir(),true)?;
            let integrations=integrations::IntegrationState::new(&data_dir);
            app.manage(Arc::new(AppState { store:Mutex::new(store), uploads:Mutex::new(attachments::Attachments::default()), integrations:tokio::sync::Mutex::new(integrations) }));
            Ok(())
        })
        .invoke_handler(|invoke| {
            if invoke.message.command().starts_with("plugin:") { return false; }
            let state=invoke.message.webview_ref().state::<Arc<AppState>>().inner().clone();
            let app=invoke.message.webview_ref().app_handle().clone();
            let command=invoke.message.command().to_owned();
            let body=invoke.message.payload().clone();
            let headers=invoke.message.headers().clone();
            invoke.resolver.respond_async(async move { dispatch(state,app,command.clone(),body,headers).await.map(|value|compat::response(&command,value)).map_err(tauri::ipc::InvokeError::from) });
            true
        })
        .run(tauri::generate_context!());
    if let Err(error)=result { upgrade::show_error(&format!("Не удалось запустить Sfery: {error}")); std::process::exit(1); }
}
