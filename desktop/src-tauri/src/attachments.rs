//! Upload protocol reconstructed and compared with an isolated original 1.44.0 process.
//! Raw comparison evidence: work/sfery/comparison/attachments-*.json.
use std::{collections::HashMap, fs, io::Write, path::PathBuf};
use serde_json::{Value, json};
use uuid::Uuid;
use crate::storage::Store;

const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 512 * 1024;
const MAX_PENDING: usize = 8;
struct Upload { name: String, size: u64, written: u64, path: PathBuf }
#[derive(Default)]
pub struct Attachments { pending: HashMap<String, Upload> }
fn error(e: impl std::fmt::Display) -> String { e.to_string() }
fn argument<T:serde::de::DeserializeOwned>(args:&Value,key:&str,command:&str)->Result<T,String>{
    let value=args.get(key).cloned().ok_or_else(||format!("invalid args `{key}` for command `{command}`: command {command} missing required key {key}"))?;
    serde_json::from_value(value).map_err(|e|format!("invalid args `{key}` for command `{command}`: {e}"))
}

impl Attachments {
    pub fn begin(&mut self, store: &Store, args: &Value) -> Result<Value, String> {
        let name: String = argument(args,"name","begin_attachment")?;
        let size: u64 = argument(args,"size","begin_attachment")?;
        if self.pending.len() >= MAX_PENDING { return Err("Дождитесь загрузки остальных файлов".into()); }
        if name.trim().is_empty() || name.chars().count() > 300 || size > MAX_FILE_BYTES { return Err("Имя файла — до 300 символов, размер — до 1 ГБ.".into()); }
        let ext = std::path::Path::new(&name).extension().and_then(|x|x.to_str()).unwrap_or("").to_lowercase();
        let suffix = if ext.len() <= 10 && ext.bytes().all(|x|x.is_ascii_alphanumeric()) && !ext.is_empty() { format!(".{ext}") } else { ".bin".into() };
        let id = format!("{}{}", Uuid::new_v4().simple(), suffix);
        let dir = store.attachments_dir();
        fs::create_dir_all(&dir).map_err(error)?;
        let path = dir.join(format!("{id}.part"));
        fs::OpenOptions::new().create_new(true).write(true).open(&path).map_err(error)?;
        self.pending.insert(id.clone(), Upload { name, size, written: 0, path });
        Ok(json!(id))
    }
    pub fn append(&mut self, id: &str, offset: u64, body: &[u8]) -> Result<Value, String> {
        let upload = self.pending.get_mut(id).ok_or("Загрузка не найдена")?;
        if offset != upload.written || body.len() > MAX_CHUNK_BYTES { return Err("Неверный размер или порядок частей файла".into()); }
        let end = offset.checked_add(body.len() as u64).ok_or("Неверный размер или порядок частей файла")?;
        if end > upload.size { return Err("Неверный размер или порядок частей файла".into()); }
        fs::OpenOptions::new().append(true).open(&upload.path).map_err(error)?.write_all(body).map_err(error)?;
        upload.written = end;
        Ok(Value::Null)
    }
    pub fn finish(&mut self, store: &mut Store, id: &str) -> Result<Value, String> {
        let upload = self.pending.get(id).ok_or("Загрузка не найдена")?;
        if upload.written != upload.size { return Err("Файл загружен не полностью".into()); }
        let path = store.attachments_dir().join(id);
        let ext = std::path::Path::new(&upload.name).extension().and_then(|x|x.to_str()).unwrap_or("").to_lowercase();
        let kind = match ext.as_str() { "png"|"jpg"|"jpeg"|"gif"|"webp"|"bmp"|"avif" => "image", "mp4"|"webm"|"mov"|"m4v" => "video", "mp3"|"wav"|"ogg"|"flac"|"m4a"|"aac" => "audio", _=>"file" };
        fs::rename(&upload.path, &path).map_err(error)?;
        if let Err(e) = store.conn.execute("INSERT INTO file_assets(id,name,kind,size) VALUES(?1,?2,?3,?4)", rusqlite::params![id,upload.name,kind,upload.size as i64]) {
            let _ = fs::rename(&path, &upload.path);
            return Err(error(e));
        }
        let result = json!({"id":id,"name":upload.name,"kind":kind,"size":upload.size,"url":"","path":path.to_string_lossy()});
        self.pending.remove(id);
        Ok(result)
    }
    pub fn discard(&mut self, store: &mut Store, args: &Value) -> Result<Value, String> {
        let ids:Vec<String> = argument(args,"ids","discard_attachments")?;
        for id in &ids {
            validate_id(id)?;
            if let Some(upload) = self.pending.remove(id) { fs::remove_file(upload.path).map_err(error)?; continue; }
            let used: bool = store.conn.query_row("SELECT EXISTS(SELECT 1 FROM task_materials WHERE asset_id=?1 UNION ALL SELECT 1 FROM area_materials WHERE asset_id=?1 UNION ALL SELECT 1 FROM project_materials WHERE asset_id=?1 UNION ALL SELECT 1 FROM subtask_materials WHERE asset_id=?1)", [id], |r|r.get(0)).map_err(error)?;
            if !used {
                let path = store.attachments_dir().join(id);
                if path.is_file() { fs::remove_file(path).map_err(error)?; }
                store.conn.execute("DELETE FROM file_assets WHERE id=?1",[id]).map_err(error)?;
            }
        }
        Ok(Value::Null)
    }
}
fn validate_id(id:&str)->Result<(),String>{
    let Some((stem,ext))=id.split_once('.') else{return Err("Некорректный файл".into())};
    if stem.len()!=32 || !stem.bytes().all(|b|b.is_ascii_hexdigit()) || ext.is_empty() || ext.len()>10 || !ext.bytes().all(|b|b.is_ascii_alphanumeric()){return Err("Некорректный файл".into());}
    Ok(())
}
pub fn asset_path(store: &Store, id: &str) -> Result<PathBuf,String> {
    validate_id(id)?;
    let exists: bool = store.conn.query_row("SELECT EXISTS(SELECT 1 FROM file_assets WHERE id=?1)",[id],|r|r.get(0)).map_err(error)?;
    if !exists { return Err("Файл не найден".into()); }
    Ok(store.attachments_dir().join(id))
}
pub fn read_text(store: &Store, id: &str) -> Result<Value,String> {
    let path = asset_path(store,id)?;
    let ext=path.extension().and_then(|s|s.to_str()).unwrap_or("");
    if !["txt","md","csv","json","log"].contains(&ext){return Err("Предпросмотр текста для этого формата недоступен".into());}
    if fs::metadata(&path).map_err(error)?.len() > MAX_TEXT_BYTES { return Err("Для файлов больше 1 МБ используйте «Открыть документ»".into()); }
    let bytes=fs::read(path).map_err(error)?;
    String::from_utf8(bytes).map(Value::String).map_err(|_|"Этот документ не в кодировке UTF-8. Используйте «Открыть документ».".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_requires_contiguous_complete_bytes() {
        let dir=std::env::temp_dir().join(format!("sfery-upload-test-{}",Uuid::new_v4()));
        let mut store=Store::new(&dir).unwrap(); let mut uploads=Attachments::default();
        let id=uploads.begin(&store,&json!({"name":"sample.txt","size":5})).unwrap().as_str().unwrap().to_owned();
        assert_eq!(id.len(),36); assert!(id[..32].bytes().all(|b|b.is_ascii_hexdigit()));
        assert!(uploads.append(&id,1,b"hello").is_err());
        uploads.append(&id,0,b"hel").unwrap();
        assert!(uploads.finish(&mut store,&id).is_err());
        uploads.append(&id,3,b"lo").unwrap();
        let material=uploads.finish(&mut store,&id).unwrap(); assert_eq!(material["kind"],"file");
        assert_eq!(read_text(&store,&id).unwrap(),"hello");
        uploads.discard(&mut store,&json!({"ids":[id]})).unwrap();
        drop(store); fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn verified_upload_limits_and_fallback_suffix() {
        let dir=std::env::temp_dir().join(format!("sfery-upload-limits-{}",Uuid::new_v4()));
        let mut store=Store::new(&dir).unwrap();let mut uploads=Attachments::default();
        assert!(uploads.begin(&store,&json!({"name":" ","size":0})).is_err());
        assert!(uploads.begin(&store,&json!({"name":"я".repeat(301),"size":0})).is_err());
        assert!(uploads.begin(&store,&json!({"name":"limit.txt","size":MAX_FILE_BYTES+1})).is_err());
        let mut ids=Vec::new();
        for _ in 0..MAX_PENDING {ids.push(uploads.begin(&store,&json!({"name":"no-extension","size":MAX_FILE_BYTES})).unwrap().as_str().unwrap().to_owned());}
        assert!(ids.iter().all(|id|id.ends_with(".bin")));
        assert_eq!(uploads.begin(&store,&json!({"name":"ninth.txt","size":0})).unwrap_err(),"Дождитесь загрузки остальных файлов");
        uploads.discard(&mut store,&json!({"ids":ids})).unwrap();
        let id=uploads.begin(&store,&json!({"name":"chunk.txt","size":MAX_CHUNK_BYTES+1})).unwrap().as_str().unwrap().to_owned();
        assert!(uploads.append(&id,0,&vec![0;MAX_CHUNK_BYTES+1]).is_err());
        uploads.discard(&mut store,&json!({"ids":[id]})).unwrap();
        drop(store);fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn verified_format_classification_and_text_errors() {
        let dir=std::env::temp_dir().join(format!("sfery-upload-formats-{}",Uuid::new_v4()));
        let mut store=Store::new(&dir).unwrap();let mut uploads=Attachments::default();
        for (name,kind) in [("image.svg","file"),("video.mkv","file"),("video.avi","file"),("video.m4v","video"),("image.avif","image")] {
            let id=uploads.begin(&store,&json!({"name":name,"size":0})).unwrap().as_str().unwrap().to_owned();
            assert_eq!(uploads.finish(&mut store,&id).unwrap()["kind"],kind);
            assert_eq!(read_text(&store,&id).unwrap_err(),"Предпросмотр текста для этого формата недоступен");
            uploads.discard(&mut store,&json!({"ids":[id]})).unwrap();
        }
        let id=uploads.begin(&store,&json!({"name":"invalid.txt","size":1})).unwrap().as_str().unwrap().to_owned();uploads.append(&id,0,&[255]).unwrap();uploads.finish(&mut store,&id).unwrap();
        assert_eq!(read_text(&store,&id).unwrap_err(),"Этот документ не в кодировке UTF-8. Используйте «Открыть документ».");
        uploads.discard(&mut store,&json!({"ids":[id]})).unwrap();
        assert!(uploads.discard(&mut store,&json!({"ids":["0123456789abcdef0123456789abcdef.txt"]})).is_ok());
        assert!(uploads.discard(&mut store,&json!({"ids":[null]})).is_err());
        drop(store);fs::remove_dir_all(dir).unwrap();
    }
}
