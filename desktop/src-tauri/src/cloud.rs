//! Cloud client reconstructed from historical routes and isolated native mock-server probes.
//! Merge algorithm adapted from the historical reconstructed desktop sync.rs.
use std::{collections::{BTreeSet,HashMap,HashSet},fs,io::Write,path::Path,sync::Mutex,time::Duration};
use serde_json::{Value,json};
use rusqlite::OptionalExtension;
use crate::storage::Store;
use super::{protect,unprotect,remove_if_present};

const DEFAULT_URL:&str="https://sfery-telegram.nrpl.chatgpt.site";
const MAX_FILE:u64=64*1024*1024;

pub async fn invoke(dir:&Path,cmd:&str,args:&Value,store:&Mutex<Store>)->Result<Value,String>{
    match cmd {
        "cloud_status"=>status(dir),
        "cloud_disconnect"=>{remove_if_present(&dir.join("cloud-recovered.bin"))?;remove_if_present(&dir.join("cloud-device.bin"))?;let db=store.lock().map_err(|_|"Хранилище недоступно")?;db.conn.execute("DELETE FROM app_settings WHERE key IN ('cloud_base','cloud_revision')",[]).map_err(|e|e.to_string())?;Ok(Value::Null)},
        "cloud_connect"=>{
            if token(dir)?.is_some(){return Err("Сначала отключите текущее соединение".into());}
            let code=args["code"].as_str().ok_or("Введите код из настроек Mini App")?.replace([' ','-'],"").to_uppercase();
            if code.len()!=10||!code.bytes().all(|c|c.is_ascii_uppercase()||(b'2'..=b'9').contains(&c)){return Err("Введите код из настроек Mini App".into());}
            let http=client()?;let value=response(http.post(format!("{}/api/pair/claim",endpoint()?)).json(&json!({"code":code})).send().await.map_err(|e|e.to_string())?).await?;
            let token=value["token"].as_str().filter(|t|valid_token(t)).ok_or("Сервер не вернул корректный код устройства")?;
            fs::create_dir_all(dir).map_err(|e|e.to_string())?;write_atomic(&credential_path(dir),&protect(token.as_bytes()).map_err(|e|e.to_string())?)?;
            let db=store.lock().map_err(|_|"Хранилище недоступно")?;db.conn.execute("DELETE FROM app_settings WHERE key IN ('cloud_base','cloud_revision')",[]).map_err(|e|e.to_string())?;
            status(dir)
        },
        "cloud_sync"=>synchronize(dir,args["resolution"].as_str(),store).await,
        _=>Err(format!("Неизвестная команда облака: {cmd}")),
    }
}

fn endpoint()->Result<String,String>{
    let raw=std::env::var("SFERY_CLOUD_URL").unwrap_or_else(|_|DEFAULT_URL.into());
    let u=reqwest::Url::parse(&raw).map_err(|_|"Некорректный адрес облака")?;
    let loopback=matches!(u.host_str(),Some("localhost"|"127.0.0.1"|"[::1]"));
    if (u.scheme()!="https"&&!(u.scheme()=="http"&&loopback))||!u.username().is_empty()||u.password().is_some()||u.query().is_some()||u.fragment().is_some(){return Err("Облачный адрес должен использовать HTTPS".into());}
    Ok(raw.trim_end_matches('/').into())
}
fn valid_token(t:&str)->bool{t.len()==64&&t.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
fn credential_path(dir:&Path)->std::path::PathBuf { dir.join(if cfg!(feature="production-upgrade") {"cloud-device.bin"} else {"cloud-recovered.bin"}) }
fn decode_token(bytes:&[u8])->Result<String,String>{
    let raw=std::str::from_utf8(bytes).map_err(|_|"Не удалось прочитать вход в облако. Подключите ПК к телефону заново.")?;
    let token=if valid_token(raw){raw.to_owned()}else{serde_json::from_str::<Value>(raw).ok().and_then(|v|v.get("token").and_then(Value::as_str).map(str::to_owned)).ok_or("Неизвестный формат входа в облако. Подключите ПК к телефону заново.")?};
    if !valid_token(&token){return Err("Подключите ПК к телефону заново".into());}Ok(token)
}
fn token(dir:&Path)->Result<Option<String>,String>{
    let primary=credential_path(dir);
    let fallback=dir.join(if cfg!(feature="production-upgrade") {"cloud-recovered.bin"} else {"cloud-device.bin"});
    let p=if primary.is_file(){primary}else if fallback.is_file(){fallback}else{return Ok(None)};
    let decrypted=unprotect(&fs::read(p).map_err(|e|e.to_string())?).map_err(|_|"Не удалось расшифровать вход в облако в этом профиле Windows.".to_string())?;
    decode_token(&decrypted).map(Some)
}
fn status(dir:&Path)->Result<Value,String>{Ok(json!({"connected":token(dir)?.is_some(),"url":endpoint()?}))}
fn client()->Result<reqwest::Client,String>{reqwest::Client::builder().timeout(Duration::from_secs(60)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|e.to_string())}
async fn response(mut r:reqwest::Response)->Result<Value,String>{let status=r.status();let bytes=bounded_body(&mut r,1_000_000).await?;let v:Value=serde_json::from_slice(&bytes).map_err(|_|format!("Сервер вернул некорректные данные ({status})"))?;if !status.is_success(){return Err(v["error"].as_str().unwrap_or("Не удалось выполнить синхронизацию").into());}Ok(v)}
async fn bounded_body(r:&mut reqwest::Response,max:u64)->Result<Vec<u8>,String>{if r.content_length().is_some_and(|n|n>max){return Err("Ответ сервера превышает допустимый размер".into());}let mut b=Vec::new();while let Some(chunk)=r.chunk().await.map_err(|e|e.to_string())?{if b.len() as u64+chunk.len() as u64>max{return Err("Ответ сервера превышает допустимый размер".into());}b.extend_from_slice(&chunk);}Ok(b)}
fn write_atomic(p:&Path,bytes:&[u8])->Result<(),String>{let tmp=p.with_extension("syncpart");let mut file=fs::File::create(&tmp).map_err(|e|e.to_string())?;file.write_all(bytes).and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;drop(file);fs::rename(&tmp,p).map_err(|e|e.to_string())}
fn empty()->Value{json!({"areas":[],"projects":[],"tasks":[],"trash":[],"kanban_stages":{}})}
fn strip_paths(v:&mut Value){match v{Value::Object(obj)=>{if let Some(Value::Array(materials))=obj.get_mut("materials"){for m in materials{if let Some(o)=m.as_object_mut(){o.remove("path");}}}for x in obj.values_mut(){strip_paths(x);}},Value::Array(rows)=>for r in rows{strip_paths(r)},_=>{}}}
fn cleaned(mut v:Value)->Value{strip_paths(&mut v);v}

async fn synchronize(dir:&Path,resolution:Option<&str>,store:&Mutex<Store>)->Result<Value,String>{
    synchronize_at_url(dir,resolution,store,&endpoint()?).await
}
async fn synchronize_at_url(dir:&Path,resolution:Option<&str>,store:&Mutex<Store>,url:&str)->Result<Value,String>{
    let token=token(dir)?.ok_or("Сначала подключите телефон")?;
    let (base,local)={let db=store.lock().map_err(|_|"Хранилище недоступно")?;
        let base=db.conn.query_row("SELECT value FROM app_settings WHERE key='cloud_base'",[],|r|r.get::<_,String>(0)).optional().map_err(|e|e.to_string())?.map(|s|serde_json::from_str(&s).map_err(|e|e.to_string())).transpose()?.unwrap_or_else(empty);
        (base,cleaned(db.snapshot()?))};
    let http=client()?;
        let got=response(http.get(format!("{url}/api/snapshot")).bearer_auth(&token).send().await.map_err(|e|e.to_string())?).await?;
        let remote=cleaned(got.get("snapshot").cloned().ok_or("Сервер не вернул планы")?);
        let revision=got["revision"].as_u64().filter(|n|*n<=9_007_199_254_740_991).ok_or("Нет версии данных")?;
        validate_snapshot(&remote)?;
        if resolution.is_some_and(|r|r!="pc"&&r!="phone"){return Err("Некорректный выбор версии".into());}
        let (merged,conflicts)=merge(&base,&local,&remote,resolution==Some("phone"))?;
        if !conflicts.is_empty(){let backups=dir.join("backups");fs::create_dir_all(&backups).map_err(|e|e.to_string())?;let p=backups.join(format!("cloud-conflict-{}.json",uuid::Uuid::new_v4()));write_atomic(&p,&serde_json::to_vec_pretty(&json!({"base":base,"pc":local,"phone":remote})).map_err(|e|e.to_string())?)?;if resolution.is_none(){return Err(conflict_message(&conflicts));}}
        validate_snapshot(&merged)?;
        transfer_files(&http,dir,&url,&token,&merged).await?;
        if serde_json::to_vec(&merged).map_err(|e|e.to_string())?.len()>900000{return Err("Превышен размер синхронизируемых данных".into());}
        {let db=store.lock().map_err(|_|"Хранилище недоступно")?;if cleaned(db.snapshot()?)!=local{return Err("Во время синхронизации появились новые изменения на ПК. Повторим при следующей проверке".into());}}
        // Original 1.44.0 does not create a revision for unchanged/import-only sync.
        // A server 409 is reported immediately, preserving both devices' documents.
        let mut new_revision=revision;
        if merged!=remote {
            let reply=http.put(format!("{url}/api/snapshot")).bearer_auth(&token).json(&json!({"snapshot":merged,"revision":revision})).send().await.map_err(|e|e.to_string())?;
            let saved=response(reply).await?;
            new_revision=saved["revision"].as_u64().ok_or("Сервер не вернул новую версию")?;
        // The server validates and canonicalizes its schema. Do not silently apply a different server document.
            if let Some(s)=saved.get("snapshot"){if cleaned(s.clone())!=merged{return Err("Сервер изменил формат планов. Локальные данные не заменены; проверьте совместимость версии сервера".into());}}
        }
        let mut db=store.lock().map_err(|_|"Хранилище недоступно")?;
        if cleaned(db.snapshot()?)!=local{return Err("Во время синхронизации появились новые изменения на ПК. Повторим при следующей проверке".into());}
        db.replace_snapshot(&merged)?;
        db.conn.execute("INSERT INTO app_settings(key,value) VALUES('cloud_base',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[serde_json::to_string(&merged).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        db.conn.execute("INSERT INTO app_settings(key,value) VALUES('cloud_revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[new_revision.to_string()]).map_err(|e|e.to_string())?;
        status(dir)
}

fn conflict_message(conflicts:&[String])->String {
    format!("Конфликт изменений: {}. На ПК и телефоне изменено одно поле. Выберите версию спорных полей в настройках подключения телефона. До выбора обе версии остаются на своих устройствах.",conflicts.first().map(String::as_str).unwrap_or("Планы"))
}

fn all_materials<'a>(v:&'a Value,out:&mut Vec<&'a Value>){match v{Value::Object(o)=>{if let Some(Value::Array(ms))=o.get("materials"){out.extend(ms);}for (key,value) in o{if key!="materials"&&key!="description_doc"{all_materials(value,out);}}},Value::Array(a)=>for x in a{all_materials(x,out)},_=>{}}}
fn valid_asset_id(id:&str)->bool{let Some((stem,ext))=id.split_once('.') else{return false};stem.len()==32&&stem.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))&&!ext.is_empty()&&ext.len()<=10&&ext.bytes().all(|b|b.is_ascii_digit()||b.is_ascii_lowercase())}
async fn transfer_files(http:&reqwest::Client,dir:&Path,url:&str,token:&str,snapshot:&Value)->Result<(),String>{
    let mut materials=Vec::new();all_materials(snapshot,&mut materials);let mut seen=HashMap::<&str,u64>::new();let attachments=dir.join("attachments");fs::create_dir_all(&attachments).map_err(|e|e.to_string())?;
    for m in materials{
        if m["kind"]=="link"{continue;}
        let id=m["id"].as_str().filter(|s|valid_asset_id(s)).ok_or("Некорректный идентификатор вложения")?;
        let size=m["size"].as_u64().filter(|n|*n<=MAX_FILE).ok_or("Файл больше 64 МБ и не может синхронизироваться")?;
        if let Some(old)=seen.insert(id,size){if old!=size{return Err("Метаданные одного вложения противоречат друг другу".into());}continue;}
        let file=attachments.join(id);let endpoint=format!("{url}/api/files/{id}");
        if file.is_file(){
            if fs::metadata(&file).map_err(|e|e.to_string())?.len()!=size{return Err(format!("Размер локального вложения {id} не совпадает"));}
            let head=http.head(&endpoint).bearer_auth(token).send().await.map_err(|e|e.to_string())?;
            if head.status()==reqwest::StatusCode::NOT_FOUND{
                let bytes=fs::read(&file).map_err(|e|e.to_string())?;if bytes.len() as u64!=size{return Err("Вложение изменилось во время синхронизации".into());}
                response(http.put(&endpoint).bearer_auth(token).header(reqwest::header::CONTENT_LENGTH,size.to_string()).body(bytes).send().await.map_err(|e|e.to_string())?).await?;
            }else if !head.status().is_success(){response(head).await?;}
        }else{
            let mut reply=http.get(&endpoint).bearer_auth(token).send().await.map_err(|e|e.to_string())?;
            if !reply.status().is_success(){response(reply).await?;return Err(format!("Вложение {id} недоступно"));}
            let bytes=bounded_body(&mut reply,size).await?;if bytes.len() as u64!=size{return Err("Размер скачанного вложения не совпадает".into());}write_atomic(&file,&bytes)?;
        }
    }Ok(())
}

fn merge_value(b:Option<&Value>,l:Option<&Value>,r:Option<&Value>,label:&str,remote:bool,conflicts:&mut BTreeSet<String>)->Option<Value>{
    if l==r{return l.cloned();}if l==b{return r.cloned();}if r==b{return l.cloned();}
    if let(Some(Value::Array(b)),Some(Value::Array(l)),Some(Value::Array(r)))=(b,l,r){
        if b.iter().chain(l).chain(r).all(|x|x.get("id").is_some()){
            let key=|v:&Value|v["id"].to_string();let bo:Vec<_>=b.iter().map(key).collect();let lo:Vec<_>=l.iter().map(key).collect();let ro:Vec<_>=r.iter().map(key).collect();
            let same=lo.len()==bo.len()&&ro.len()==bo.len()&&lo.iter().chain(&ro).all(|id|bo.contains(id));
            if same&&lo!=bo&&ro!=bo&&lo!=ro{conflicts.insert(format!("{label} — порядок"));}
            let order=if lo==bo||remote&&same&&ro!=bo{&ro}else{&lo};let mut ids=Vec::new();for id in order.iter().chain(&lo).chain(&ro).chain(&bo){if !ids.contains(id){ids.push(id.clone());}}
            return Some(Value::Array(ids.iter().filter_map(|id|{let bv=b.iter().find(|x|key(x)==*id);let lv=l.iter().find(|x|key(x)==*id);let rv=r.iter().find(|x|key(x)==*id);let row_label=format!("{label} #{}",id.trim_matches('"'));merge_value(bv,lv,rv,&row_label,remote,conflicts)}).collect()));
        }
    }
    if let(Some(Value::Object(b)),Some(Value::Object(l)),Some(Value::Object(r)))=(b,l,r){let keys:BTreeSet<_>=b.keys().chain(l.keys()).chain(r.keys()).collect();let mut out=serde_json::Map::new();for k in keys{let field_label=if label.is_empty(){k.to_string()}else{format!("{label}.{k}")};let v=if k=="updated_at"{[l.get(k),r.get(k)].into_iter().flatten().filter_map(Value::as_str).max().map(|v|json!(v))}else{merge_value(b.get(k),l.get(k),r.get(k),&field_label,remote,conflicts)};if let Some(v)=v{out.insert(k.clone(),v);}}return Some(Value::Object(out));}
    conflicts.insert(label.into());if remote{r.cloned()}else{l.cloned()}
}

fn combined(s:&Value)->Result<Value,String>{let mut s=cleaned(s.clone());let trash=s["trash"].as_array().ok_or("Некорректная корзина")?.clone();s["tasks"].as_array_mut().ok_or("Некорректные задачи")?.extend(trash);s["trash"]=json!([]);Ok(s)}
fn merge(base:&Value,local:&Value,remote:&Value,prefer_remote:bool)->Result<(Value,Vec<String>),String>{
    let mut conflicts=BTreeSet::new();let (base,localc,remotec)=(combined(base)?,combined(local)?,combined(remote)?);
    let mut out=merge_value(Some(&base),Some(&localc),Some(&remotec),"",prefer_remote,&mut conflicts).ok_or("Не удалось объединить планы")?;
    let mut tasks=Vec::new();let mut trash=Vec::new();for task in out["tasks"].as_array().ok_or("Некорректные задачи")?.iter().cloned(){if task.get("deleted_at").is_some_and(|d|!d.is_null()){trash.push(task)}else{tasks.push(task)}}out["tasks"]=json!(tasks);out["trash"]=json!(trash);
    let preferred=if prefer_remote{remote}else{local};
    let projects=out["projects"].as_array().ok_or("Некорректные проекты")?.clone();
    for p in &projects{if !out["areas"].as_array().unwrap().iter().any(|a|a["id"]==p["area_id"]){conflicts.insert(format!("{} — сфера удалена на другом устройстве",p["name"].as_str().unwrap_or("Проект")));let pp=preferred["projects"].as_array().and_then(|a|a.iter().find(|x|x["id"]==p["id"]&&x["area_id"]==p["area_id"]));if pp.is_some(){if let Some(a)=preferred["areas"].as_array().and_then(|a|a.iter().find(|a|a["id"]==p["area_id"])){out["areas"].as_array_mut().unwrap().push(a.clone());}}}}
    let area_ids:HashSet<_>=out["areas"].as_array().unwrap().iter().map(|a|a["id"].to_string()).collect();
    out["projects"]=json!(projects.into_iter().filter(|p|area_ids.contains(&p["area_id"].to_string())).collect::<Vec<_>>());
    let project_areas:HashMap<_,_>=out["projects"].as_array().unwrap().iter().map(|p|(p["id"].to_string(),p["area_id"].clone())).collect();
    for key in ["tasks","trash"]{for t in out[key].as_array_mut().unwrap(){if !t["area_id"].is_null()&&!area_ids.contains(&t["area_id"].to_string()){t["area_id"]=Value::Null;t["project_id"]=Value::Null;}if !t["project_id"].is_null()&&project_areas.get(&t["project_id"].to_string())!=Some(&t["area_id"]){t["project_id"]=Value::Null;}}}
    let tids:HashSet<_>=out["tasks"].as_array().unwrap().iter().chain(out["trash"].as_array().unwrap()).map(|t|t["id"].to_string()).collect();if let Some(k)=out["kanban_stages"].as_object_mut(){k.retain(|id,_|tids.contains(id));}
    Ok((out,conflicts.into_iter().collect()))
}

fn validate_snapshot(v:&Value)->Result<(),String>{
    let limits=[("areas",1000),("projects",2000),("tasks",5000),("trash",5000)];
    for (key,max) in limits{let rows=v[key].as_array().filter(|a|a.len()<=max).ok_or_else(||format!("Некорректный список {key}"))?;let mut ids=HashSet::new();for row in rows{let id=row["id"].as_i64().filter(|i|*i>0&&*i<=9_007_199_254_740_991).ok_or("Некорректный идентификатор")?;if !ids.insert(id){return Err("Повторяющиеся идентификаторы".into());}let title=row.get("title").or(row.get("name")).and_then(Value::as_str).ok_or("Нет названия записи")?;if title.trim().is_empty()||title.chars().count()>300{return Err("Название должно содержать от 1 до 300 символов".into());}}}
    let areas:HashSet<_>=v["areas"].as_array().unwrap().iter().map(|a|a["id"].clone()).collect();let projects:HashMap<_,_>=v["projects"].as_array().unwrap().iter().map(|p|(p["id"].clone(),p["area_id"].clone())).collect();
    if projects.values().any(|a|!areas.contains(a)){return Err("Сфера проекта не найдена".into());}
    let mut tids=HashSet::new();for key in ["tasks","trash"]{for t in v[key].as_array().unwrap(){if !tids.insert(t["id"].to_string()){return Err("Задача присутствует и в списке, и в корзине".into());}if !t["area_id"].is_null()&&!areas.contains(&t["area_id"])||!t["project_id"].is_null()&&projects.get(&t["project_id"])!=Some(&t["area_id"]){return Err("Сфера или проект задачи не найдены".into());}let deleted=t.get("deleted_at").is_some_and(|d|!d.is_null());if (key=="trash")!=deleted{return Err("Некорректная корзина".into());}}}
    let stages=v["kanban_stages"].as_object().ok_or("Некорректные колонки")?;if stages.iter().any(|(id,s)|!tids.contains(id)||!matches!(s.as_str(),Some("progress"|"review"))){return Err("Некорректная колонка".into());}
    let mut materials=Vec::new();all_materials(v,&mut materials);for m in materials{let kind=m["kind"].as_str().ok_or("Некорректный тип вложения")?;if !matches!(kind,"image"|"video"|"audio"|"file"|"link"){return Err("Некорректный тип вложения".into());}if kind=="link"{if !m["url"].as_str().is_some_and(|u|u.starts_with("https://")||u.starts_with("http://")){return Err("Некорректная ссылка".into());}}else if !m["id"].as_str().is_some_and(valid_asset_id)||m["url"].as_str()!=Some(""){return Err("Некорректное вложение".into());}}
    Ok(())
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn legacy_cloud_credential_formats(){let value="a".repeat(64);assert_eq!(decode_token(value.as_bytes()).unwrap(),value);assert_eq!(decode_token(json!({"token":value}).to_string().as_bytes()).unwrap(),value);assert!(decode_token(b"{\"token\":\"invalid\"}").is_err());}
    #[cfg(windows)]#[test]fn legacy_cloud_file_remains_unchanged(){let dir=std::env::temp_dir().join(format!("sfery-legacy-cloud-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&dir).unwrap();let bytes=protect("a".repeat(64).as_bytes()).unwrap();fs::write(dir.join("cloud-device.bin"),&bytes).unwrap();assert_eq!(token(&dir).unwrap(),Some("a".repeat(64)));assert_eq!(fs::read(dir.join("cloud-device.bin")).unwrap(),bytes);fs::remove_dir_all(dir).unwrap();}
    fn task(title:&str,description:&str)->Value{json!({"id":1,"area_id":null,"project_id":null,"title":title,"description":description,"updated_at":"2026-10-16T00:00:00Z","deleted_at":null,"subtasks":[],"materials":[]})}
    fn snapshot(t:Value)->Value{let mut s=empty();s["tasks"]=json!([t]);s}
    #[test]fn independent_changes_merge(){let b=snapshot(task("А","Б"));let l=snapshot(task("Новый","Б"));let r=snapshot(task("А","Текст"));let (v,c)=merge(&b,&l,&r,false).unwrap();assert!(c.is_empty());assert_eq!(v["tasks"][0]["title"],"Новый");assert_eq!(v["tasks"][0]["description"],"Текст");}
    #[test]fn conflicts_require_resolution(){let b=snapshot(task("А","Б"));let l=snapshot(task("ПК","Б"));let r=snapshot(task("Телефон","Б"));let(v,c)=merge(&b,&l,&r,true).unwrap();assert!(!c.is_empty());assert_eq!(v["tasks"][0]["title"],"Телефон");}
    #[test]fn original_conflict_reports_first_field_path(){let b=snapshot(task("А","Б"));let l=snapshot(task("ПК","ПК текст"));let r=snapshot(task("Телефон","Телефон текст"));let(_,c)=merge(&b,&l,&r,false).unwrap();assert_eq!(c,vec!["tasks #1.description","tasks #1.title"]);assert_eq!(conflict_message(&c),"Конфликт изменений: tasks #1.description. На ПК и телефоне изменено одно поле. Выберите версию спорных полей в настройках подключения телефона. До выбора обе версии остаются на своих устройствах.");}
    #[test]fn trash_movement_preserves_other_changes(){let b=snapshot(task("А","Б"));let l=snapshot(task("Новое имя","Б"));let mut r=empty();let mut deleted=task("А","Б");deleted["deleted_at"]=json!("2026-10-16T02:00:00Z");r["trash"]=json!([deleted]);let(v,c)=merge(&b,&l,&r,false).unwrap();assert!(c.is_empty());assert!(v["tasks"].as_array().unwrap().is_empty());assert_eq!(v["trash"][0]["title"],"Новое имя");}
    #[test]fn strip_only_material_paths(){let mut v=snapshot(task("А","Б"));v["tasks"][0]["materials"]=json!([{"id":"abc","path":"private-file"}]);strip_paths(&mut v);assert!(v["tasks"][0]["materials"][0].get("path").is_none());}
    #[test]fn invalid_asset_paths_rejected(){assert!(!valid_asset_id("../../secret"));assert!(!valid_asset_id("a"));assert!(valid_asset_id("0123456789abcdef0123456789abcdef.png"));}
    #[cfg(windows)]
    fn mock_sync_contract(local_change:bool,conflict_response:bool)->(Result<Value,String>,Vec<String>){
        use std::io::{BufRead,Read};
        let dir=std::env::temp_dir().join(format!("sfery-cloud-contract-{}",uuid::Uuid::new_v4()));
        let mut db=Store::new(&dir).unwrap();
        if local_change{db.invoke("save_task",json!({"task":{"id":0,"title":"Local revision race"}})).unwrap();}
        let before=db.snapshot().unwrap();let store=Mutex::new(db);
        write_atomic(&dir.join("cloud-recovered.bin"),&protect("a".repeat(64).as_bytes()).unwrap()).unwrap();
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let url=format!("http://{}",listener.local_addr().unwrap());
        let worker=std::thread::spawn(move||{
            let mut requests=Vec::new();
            for _ in 0..(if local_change{2}else{1}){
                let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut reader=std::io::BufReader::new(stream.try_clone().unwrap());let mut first=String::new();reader.read_line(&mut first).unwrap();let mut size=0usize;
                loop{let mut line=String::new();reader.read_line(&mut line).unwrap();if line=="\r\n"||line.is_empty(){break;}if let Some(n)=line.to_lowercase().strip_prefix("content-length:"){size=n.trim().parse().unwrap();}}
                let mut body=vec![0;size];reader.read_exact(&mut body).unwrap();
                let(status,reply)=if first.starts_with("GET /api/snapshot "){("200 OK",json!({"snapshot":empty(),"revision":7}))}else{assert!(first.starts_with("PUT /api/snapshot "));assert!(conflict_response);("409 Conflict",json!({"error":"mock concurrent update"}))};
                let payload=serde_json::to_vec(&reply).unwrap();write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",payload.len()).unwrap();stream.write_all(&payload).unwrap();requests.push(first);
            }requests
        });
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result=runtime.block_on(synchronize_at_url(&dir,None,&store,&url));
        assert_eq!(store.lock().unwrap().snapshot().unwrap(),before);
        if !local_change{assert_eq!(store.lock().unwrap().setting("cloud_revision").unwrap().as_deref(),Some("7"));}
        else{assert!(store.lock().unwrap().setting("cloud_base").unwrap().is_none());}
        let requests=worker.join().unwrap();drop(store);fs::remove_dir_all(dir).unwrap();(result,requests)
    }
    #[cfg(windows)]
    #[test]fn unchanged_cloud_sync_does_not_put_snapshot(){let(result,requests)=mock_sync_contract(false,false);assert_eq!(result.unwrap()["connected"],true);assert_eq!(requests.len(),1);assert!(requests[0].starts_with("GET /api/snapshot "));}
    #[cfg(windows)]
    #[test]fn original_409_error_preserves_local_changes_without_retry(){let(result,requests)=mock_sync_contract(true,true);assert_eq!(result.unwrap_err(),"mock concurrent update");assert_eq!(requests.len(),2);}
    #[cfg(windows)]
    #[test]fn mock_server_synchronizes_without_production_requests(){
        use std::io::{BufRead,Read};
        let dir=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../work/sfery/integration-tests").join(uuid::Uuid::new_v4().to_string());
        let mut db=Store::new(&dir).unwrap();
        db.invoke("save_task",json!({"task":{"id":0,"title":"Локальная задача"}})).unwrap();
        let local=cleaned(db.snapshot().unwrap());let store=Mutex::new(db);
        write_atomic(&dir.join("cloud-recovered.bin"),&protect("a".repeat(64).as_bytes()).unwrap()).unwrap();
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let url=format!("http://{}",listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker=std::thread::spawn(move||{
            let deadline=std::time::Instant::now()+Duration::from_secs(10);let mut requests=Vec::new();
            while requests.len()<2&&std::time::Instant::now()<deadline{
                let(mut stream,_)=match listener.accept(){Ok(s)=>s,Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(5));continue},Err(e)=>panic!("{e}")};
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut reader=std::io::BufReader::new(stream.try_clone().unwrap());let mut first=String::new();reader.read_line(&mut first).unwrap();let mut content_length=0;let mut bearer=false;
                loop{let mut line=String::new();reader.read_line(&mut line).unwrap();if line=="\r\n"||line.is_empty(){break;}let lower=line.to_lowercase();if let Some(s)=lower.strip_prefix("content-length:"){content_length=s.trim().parse::<usize>().unwrap();}if lower.starts_with("authorization: bearer "){bearer=true;}}
                assert!(bearer);let mut body=vec![0;content_length];reader.read_exact(&mut body).unwrap();
                let reply=if first.starts_with("GET /api/snapshot "){json!({"snapshot":empty(),"revision":0})}else{assert!(first.starts_with("PUT /api/snapshot "));let v:Value=serde_json::from_slice(&body).unwrap();assert_eq!(v["revision"],0);json!({"snapshot":v["snapshot"],"revision":1})};
                let encoded=serde_json::to_vec(&reply).unwrap();write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",encoded.len()).unwrap();stream.write_all(&encoded).unwrap();requests.push(first);
            }
            assert_eq!(requests.len(),2);requests
        });
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result=runtime.block_on(synchronize_at_url(&dir,None,&store,&url)).unwrap();assert_eq!(result["connected"],true);
        assert_eq!(cleaned(store.lock().unwrap().snapshot().unwrap()),local);assert!(store.lock().unwrap().setting("cloud_base").unwrap().is_some());worker.join().unwrap();
    }
}
