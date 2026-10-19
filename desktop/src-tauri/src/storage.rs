//! Reimplemented local planner operations from recovered SQL and frontend contracts.
//! This is maintainable reconstruction, not an assertion of original Rust control flow.
use crate::models::{Area, Material, Project, Subtask, Task};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, fs, path::{Path, PathBuf}};

type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display + 'static) -> String {
    if (&e as &dyn std::any::Any).is::<rusqlite::Error>() {
        format!("Не удалось сохранить или прочитать данные: {e}")
    } else {e.to_string()}
}
fn title(s: &str) -> Result<&str> {
    let s=s.trim();
    if s.is_empty() || s.chars().count()>300 { Err("Название должно содержать от 1 до 300 символов".into()) } else { Ok(s) }
}
fn id_arg(a:&Value)->Result<i64>{a["id"].as_i64().ok_or_else(||"Некорректный идентификатор".into())}
fn url(s:&str)->Result<()> {
    if s.is_empty() || ((s.starts_with("https://")||s.starts_with("http://")) && !s.chars().any(char::is_control) && s.split("://").nth(1).is_some_and(|h|!h.is_empty())) {Ok(())} else {Err("Разрешены только ссылки http:// и https://".into())}
}
fn due(v:Option<&str>)->Result<()>{if let Some(v)=v{if v.len()!=10{return Err("Некорректная дата".into());}chrono::NaiveDate::parse_from_str(v,"%Y-%m-%d").map_err(|_|"Некорректная дата".to_string())?;}Ok(())}

pub struct Store { pub conn:Connection, pub data_dir:PathBuf }
impl Store {
    pub fn new(data_dir:&Path)->Result<Self>{
        fs::create_dir_all(data_dir.join("attachments")).map_err(err)?;
        let conn=Connection::open(data_dir.join("planner.sqlite")).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(err)?;
        crate::upgrade::validate_database(&conn)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;").map_err(err)?;
        let migration=conn.unchecked_transaction().map_err(err)?;
        let tasks:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='tasks')",[],|r|r.get(0)).map_err(err)?;
        if tasks {
            let has:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name='completed_at')",[],|r|r.get(0)).map_err(err)?;
            if !has {conn.execute_batch("ALTER TABLE tasks ADD COLUMN completed_at TEXT; UPDATE tasks SET completed_at=updated_at WHERE completed=1;").map_err(err)?;}
        }
        let schema=include_str!("schema.sql").replace("CREATE TABLE ","CREATE TABLE IF NOT EXISTS ").replace("CREATE INDEX ","CREATE INDEX IF NOT EXISTS ").replace("CREATE TRIGGER ","CREATE TRIGGER IF NOT EXISTS ");
        conn.execute_batch(&schema).map_err(err)?;
        conn.execute("INSERT OR IGNORE INTO app_settings(key,value) VALUES('schema_version','9')",[]).map_err(err)?;
        for (column,ddl) in [("confidence","ALTER TABLE telegram_candidates ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0"),("bucket","ALTER TABLE telegram_candidates ADD COLUMN bucket TEXT NOT NULL DEFAULT 'high'")] {
            let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('telegram_candidates') WHERE name=?1)",[column],|r|r.get(0)).map_err(err)?;
            if !exists{conn.execute_batch(ddl).map_err(err)?;}
        }
        // Exact reserve/high-water trigger definitions were subsequently recovered
        // from sqlite_schema in a native isolated-app backup. Remove only obsolete
        // trigger names from the earlier reconstruction, preserving all reservations.
        for kind in ["areas","projects","tasks"] {
            conn.execute(&format!("INSERT OR IGNORE INTO allocated_ids(kind,id) SELECT ?1,id FROM {kind}"),[kind]).map_err(err)?;
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS recovered_{kind}_reserve_id;")).map_err(err)?;
            conn.execute(&format!("INSERT INTO app_settings(key,value) SELECT ?1,CAST(COALESCE(MAX(id),0) AS TEXT) FROM {kind} WHERE true ON CONFLICT(key) DO UPDATE SET value=CAST(MAX(CAST(app_settings.value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)"),[format!("id_high_water_{kind}")]).map_err(err)?;
        }
        conn.execute("UPDATE app_settings SET value='9' WHERE key='schema_version'",[]).map_err(err)?;
        migration.commit().map_err(err)?;
        Ok(Self{conn,data_dir:data_dir.to_path_buf()})
    }
    pub fn attachments_dir(&self)->PathBuf{self.data_dir.join("attachments")}
    pub fn setting(&self,key:&str)->Result<Option<String>>{self.conn.query_row("SELECT value FROM app_settings WHERE key=?1",[key],|r|r.get(0)).optional().map_err(err)}
    pub fn set_setting(&self,key:&str,value:&str)->Result<()>{self.conn.execute("INSERT INTO app_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value]).map_err(err)?;Ok(())}
    fn transaction(&self,f:impl FnOnce()->Result<Value>)->Result<Value>{let tx=self.conn.unchecked_transaction().map_err(err)?;let v=f()?;tx.commit().map_err(err)?;Ok(v)}
    fn alloc(&self,kind:&str)->Result<i64>{
        // Random safe-integer IDs avoid independent devices generating the same sequence.
        // Original allocator control flow is unknown; reservation/nonreuse is observed SQL.
        for _ in 0..32 {
            let bytes=uuid::Uuid::new_v4();let raw=u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap());
            let v=(raw%9_007_199_254_740_991+1) as i64;
            if self.conn.execute("INSERT OR IGNORE INTO allocated_ids(kind,id) VALUES(?1,?2)",params![kind,v]).map_err(err)?==1{return Ok(v);}
        }Err("Не удалось выделить новый идентификатор записи".into())
    }
    fn exists(&self,table:&str,id:i64)->Result<bool>{self.conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),[id],|r|r.get(0)).map_err(err)}
    fn active(&self,id:i64)->Result<()> {
        let yes:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM task_trash WHERE task_id=?1))",[id],|r|r.get(0)).map_err(err)?;
        if yes {Ok(())} else {Err("Запись уже удалена. Обновите список.".into())}
    }
    fn materials(&self,table:&str,key:&str,id:i64,sub:Option<i64>)->Result<Vec<Material>>{
        let sql=format!("SELECT content FROM {table} WHERE {key}=?1{} ORDER BY position",if sub.is_some(){" AND subtask_position=?2"}else{""});
        let mut q=self.conn.prepare(&sql).map_err(err)?;
        let mut rows=if let Some(s)=sub{q.query(params![id,s])}else{q.query([id])}.map_err(err)?;
        let mut out=vec![];while let Some(r)=rows.next().map_err(err)?{let raw:String=r.get(0).map_err(err)?;let mut m:Material=serde_json::from_str(&raw).map_err(err)?;if m.kind!="link"{m.path=Some(self.attachments_dir().join(&m.id).to_string_lossy().into_owned());}out.push(m);}Ok(out)
    }
    fn validate_materials(&self,ms:&[Material])->Result<()> {
        let mut seen=HashSet::new();for m in ms {
            if m.id.is_empty()||m.id.len()>128||m.id.starts_with('.')||m.id.contains("..")||!m.id.chars().all(|c|c.is_ascii_alphanumeric()||c=='-'||c=='_'||c=='.') {return Err("Некорректный идентификатор материала".into());}
            if !seen.insert(&m.id){return Err("Материал добавлен дважды".into());}title(&m.name)?;
            if m.kind=="link"{if m.url.is_empty(){return Err("Ссылка отсутствует".into());}url(&m.url)?;}else{
                let found:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM file_assets WHERE id=?1 AND kind=?2 AND size=?3)",params![m.id,m.kind,i64::try_from(m.size).map_err(|_|"Слишком большой размер файла")?],|r|r.get(0)).map_err(err)?;
                if !found{return Err("Файл недоступен. Добавьте его заново.".into());}
            }
        }Ok(())
    }
    fn save_materials(&self,table:&str,key:&str,id:i64,sub:Option<i64>,ms:&[Material])->Result<()> {
        self.validate_materials(ms)?;
        if let Some(s)=sub {self.conn.execute(&format!("DELETE FROM {table} WHERE {key}=?1 AND subtask_position=?2"),params![id,s]).map_err(err)?;} else {self.conn.execute(&format!("DELETE FROM {table} WHERE {key}=?1"),[id]).map_err(err)?;}
        for (p,m) in ms.iter().enumerate(){let mut m=m.clone();m.path=None;let raw=serde_json::to_string(&m).map_err(err)?;let asset=if m.kind=="link"{None}else{Some(m.id)};
            if let Some(s)=sub {self.conn.execute(&format!("INSERT INTO {table}({key},subtask_position,position,content,asset_id) VALUES(?1,?2,?3,?4,?5)"),params![id,s,p as i64,raw,asset]).map_err(err)?;} else {self.conn.execute(&format!("INSERT INTO {table}({key},position,content,asset_id) VALUES(?1,?2,?3,?4)"),params![id,p as i64,raw,asset]).map_err(err)?;}
        }Ok(())
    }
    fn validate_doc(v:&Value,depth:usize)->Result<()> {
        if depth>32{return Err("Слишком много вложенных списков".into());}
        let t=v["type"].as_str().ok_or("Некорректная структура описания")?;
        if !["doc","text","paragraph","heading","bulletList","orderedList","taskList","blockquote","codeBlock","horizontalRule","listItem","taskItem","hardBreak"].contains(&t){return Err("Неподдерживаемое содержимое описания".into());}
        if let Some(link)=v.pointer("/attrs/href").and_then(Value::as_str){url(link)?;}
        if let Some(ms)=v["marks"].as_array(){for m in ms{if let Some(link)=m.pointer("/attrs/href").and_then(Value::as_str){url(link)?;}}}
        if let Some(nodes)=v["content"].as_array(){for n in nodes{Self::validate_doc(n,depth+1)?;}}Ok(())
    }
    fn save_task(&self,t:&Task)->Result<Value>{
        let mut t=t.clone();
        if t.id==0&&t.due_date.is_none(){t.due_date=Some(chrono::Local::now().format("%Y-%m-%d").to_string());}
        for sub in &mut t.subtasks{if sub.id.is_empty(){sub.id=uuid::Uuid::new_v4().to_string();}}
        title(&t.title)?;url(&t.url)?;due(t.due_date.as_deref())?;
        self.validate_materials(&t.materials)?;let mut keys=HashSet::new();for s in &t.subtasks {title(&s.title)?;if s.id.is_empty()||s.id.len()>128||!keys.insert(&s.id){return Err("Некорректный идентификатор подзадачи".into());}self.validate_materials(&s.materials)?;}
        if let Some(v)=&t.description_doc{if v.to_string().len()>1024*1024{return Err("Слишком большое описание".into());}Self::validate_doc(v,0)?;}
        self.transaction(||{
            let id=if t.id==0{let id=self.alloc("tasks")?;self.conn.execute("INSERT INTO tasks(id,area_id,project_id,title,description,url,due_date,completed) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,t.area_id,t.project_id,t.title.trim(),t.description,t.url,t.due_date,t.completed]).map_err(err)?;self.conn.execute("INSERT INTO task_order(task_id,position) SELECT ?1,COALESCE(min(position),0)-1 FROM task_order",[id]).map_err(err)?;id}else{self.active(t.id)?;self.conn.execute("UPDATE tasks SET area_id=?1,project_id=?2,title=?3,description=?4,url=?5,due_date=?6,completed=?7,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?8",params![t.area_id,t.project_id,t.title.trim(),t.description,t.url,t.due_date,t.completed,t.id]).map_err(err)?;t.id};
            self.save_materials("task_materials","task_id",id,None,&t.materials)?;
            self.conn.execute("DELETE FROM subtasks WHERE task_id=?1",[id]).map_err(err)?;
            for (p,s) in t.subtasks.iter().enumerate(){self.conn.execute("INSERT INTO subtasks(task_id,position,title,completed) VALUES(?1,?2,?3,?4)",params![id,p as i64,s.title.trim(),s.completed]).map_err(err)?;self.conn.execute("INSERT INTO subtask_keys(task_id,subtask_position,id) VALUES(?1,?2,?3)",params![id,p as i64,s.id]).map_err(err)?;self.save_materials("subtask_materials","task_id",id,Some(p as i64),&s.materials)?;}
            self.conn.execute("DELETE FROM task_rich_descriptions WHERE task_id=?1",[id]).map_err(err)?;if let Some(v)=&t.description_doc{self.conn.execute("INSERT INTO task_rich_descriptions(task_id,content) VALUES(?1,?2)",params![id,v.to_string()]).map_err(err)?;}
            Ok(json!(id))
        })
    }
    pub fn snapshot(&self)->Result<Value>{
        let mut q=self.conn.prepare("SELECT id,name,icon,color,created_at FROM areas ORDER BY id").map_err(err)?;
        let rows=q.query_map([],|r|Ok(Area{id:r.get(0)?,name:r.get(1)?,icon:r.get(2)?,color:r.get(3)?,created_at:r.get(4)?,materials:vec![]})).map_err(err)?;
        let mut areas=vec![];for r in rows{let mut a=r.map_err(err)?;a.materials=self.materials("area_materials","area_id",a.id,None)?;areas.push(a);}
        let mut q=self.conn.prepare("SELECT id,area_id,name,created_at FROM projects ORDER BY id").map_err(err)?;
        let rows=q.query_map([],|r|Ok(Project{id:r.get(0)?,area_id:r.get(1)?,name:r.get(2)?,created_at:r.get(3)?,materials:vec![]})).map_err(err)?;
        let mut projects=vec![];for r in rows{let mut p=r.map_err(err)?;p.materials=self.materials("project_materials","project_id",p.id,None)?;projects.push(p);}
        let mut q=self.conn.prepare("SELECT t.id,t.area_id,t.project_id,t.title,t.description,t.url,t.due_date,t.completed,t.created_at,t.updated_at,o.position,t.completed_at,tr.deleted_at FROM tasks t LEFT JOIN task_order o ON o.task_id=t.id LEFT JOIN task_trash tr ON tr.task_id=t.id ORDER BY COALESCE(o.position,0),t.created_at DESC,t.id DESC").map_err(err)?;
        let rows=q.query_map([],|r|Ok(Task{id:r.get(0)?,area_id:r.get(1)?,project_id:r.get(2)?,title:r.get(3)?,description:r.get(4)?,url:r.get(5)?,due_date:r.get(6)?,completed:r.get(7)?,created_at:r.get(8)?,updated_at:r.get(9)?,sort_order:r.get(10)?,completed_at:r.get(11)?,deleted_at:r.get(12)?,..Task::default()})).map_err(err)?;
        let (mut tasks,mut trash)=(vec![],vec![]);for r in rows{let mut t=r.map_err(err)?;t.materials=self.materials("task_materials","task_id",t.id,None)?;
            let raw:Option<String>=self.conn.query_row("SELECT content FROM task_rich_descriptions WHERE task_id=?1",[t.id],|r|r.get(0)).optional().map_err(err)?;t.description_doc=raw.map(|s|serde_json::from_str(&s).map_err(err)).transpose()?;
            let mut sq=self.conn.prepare("SELECT s.position,s.title,s.completed,COALESCE(k.id,'legacy-'||s.task_id||'-'||s.position) FROM subtasks s LEFT JOIN subtask_keys k ON k.task_id=s.task_id AND k.subtask_position=s.position WHERE s.task_id=?1 ORDER BY s.position").map_err(err)?;
            let subs=sq.query_map([t.id],|r|Ok((r.get::<_,i64>(0)?,Subtask{id:r.get(3)?,title:r.get(1)?,completed:r.get(2)?,materials:vec![]}))).map_err(err)?;for s in subs{let (p,mut s)=s.map_err(err)?;s.materials=self.materials("subtask_materials","task_id",t.id,Some(p))?;t.subtasks.push(s);}
            if t.deleted_at.is_some(){trash.push(t)}else{tasks.push(t)}
        }
        let mut q=self.conn.prepare("SELECT task_id,stage FROM task_kanban_stage ORDER BY task_id").map_err(err)?;
        let stages=q.query_map([],|r|Ok((r.get::<_,i64>(0)?.to_string(),r.get::<_,String>(1)?))).map_err(err)?.collect::<rusqlite::Result<HashMap<_,_>>>().map_err(err)?;
        Ok(json!({"areas":areas,"projects":projects,"tasks":tasks,"trash":trash,"kanban_stages":stages}))
    }
    /// Replace relational planner data atomically, keeping application settings and
    /// Telegram provenance for surviving task IDs. Network transfer happens outside Store.
    pub fn replace_snapshot(&mut self,value:&Value)->Result<()> {
        fn list<T:serde::de::DeserializeOwned>(v:&Value,key:&str)->Result<Vec<T>>{serde_json::from_value(v.get(key).cloned().unwrap_or(json!([]))).map_err(err)}
        let areas:Vec<Area>=list(value,"areas")?;let projects:Vec<Project>=list(value,"projects")?;
        let mut tasks:Vec<Task>=list(value,"tasks")?;let trash:Vec<Task>=list(value,"trash")?;
        let active_count=tasks.len();tasks.extend(trash);
        let stages:HashMap<String,String>=serde_json::from_value(value.get("kanban_stages").cloned().unwrap_or(json!({}))).map_err(err)?;
        fn unique(ids:impl Iterator<Item=i64>)->Result<HashSet<i64>>{let mut out=HashSet::new();for id in ids{if id<=0||id>9_007_199_254_740_991||!out.insert(id){return Err("Некорректный или повторяющийся идентификатор".into());}}Ok(out)}
        let area_ids=unique(areas.iter().map(|x|x.id))?;let project_ids=unique(projects.iter().map(|x|x.id))?;let task_ids=unique(tasks.iter().map(|x|x.id))?;
        for a in &areas{title(&a.name)?;if a.color.len()!=7||!a.color.starts_with('#')||!a.color[1..].chars().all(|c|c.is_ascii_hexdigit()){return Err("Некорректный цвет".into());}}
        for p in &projects{title(&p.name)?;if !area_ids.contains(&p.area_id){return Err("Сфера проекта отсутствует".into());}}
        for (i,t) in tasks.iter().enumerate(){title(&t.title)?;url(&t.url)?;due(t.due_date.as_deref())?;if t.area_id.is_some_and(|id|!area_ids.contains(&id))||t.project_id.is_some_and(|id|!projects.iter().any(|p|p.id==id&&Some(p.area_id)==t.area_id)){return Err("Сфера или проект задачи отсутствует".into());}if (i<active_count)!=t.deleted_at.is_none(){return Err("Некорректная корзина".into());}let mut seen=HashSet::new();for s in &t.subtasks{title(&s.title)?;if s.id.is_empty()||!seen.insert(&s.id){return Err("Некорректная подзадача".into());}}if let Some(doc)=&t.description_doc{if doc.to_string().len()>1024*1024{return Err("Слишком большое описание".into());}Self::validate_doc(doc,0)?;}}
        for (id,stage) in &stages {let id=id.parse::<i64>().map_err(err)?;if !task_ids.contains(&id)||!["progress","review"].contains(&stage.as_str()){return Err("Некорректная колонка".into());}}
        self.transaction(||{
            self.conn.execute_batch("PRAGMA defer_foreign_keys=ON;").map_err(err)?;
            let all_materials=areas.iter().flat_map(|a|a.materials.iter()).chain(projects.iter().flat_map(|p|p.materials.iter())).chain(tasks.iter().flat_map(|t|t.materials.iter().chain(t.subtasks.iter().flat_map(|s|s.materials.iter()))));
            for m in all_materials{if m.kind!="link"{self.conn.execute("INSERT INTO file_assets(id,name,kind,size) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET name=excluded.name,kind=excluded.kind,size=excluded.size",params![m.id,m.name,m.kind,i64::try_from(m.size).map_err(|_|"Слишком большой размер файла")?]).map_err(err)?;}}
            for a in &areas {self.conn.execute("INSERT INTO areas(id,name,icon,color,created_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET name=excluded.name,icon=excluded.icon,color=excluded.color",params![a.id,a.name,a.icon,a.color,a.created_at]).map_err(err)?;self.save_materials("area_materials","area_id",a.id,None,&a.materials)?;}
            for p in &projects {self.conn.execute("INSERT INTO projects(id,area_id,name,created_at) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET area_id=excluded.area_id,name=excluded.name",params![p.id,p.area_id,p.name,p.created_at]).map_err(err)?;self.save_materials("project_materials","project_id",p.id,None,&p.materials)?;}
            for t in &tasks {
                self.conn.execute("INSERT INTO tasks(id,area_id,project_id,title,description,url,due_date,completed,created_at,updated_at,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET area_id=excluded.area_id,project_id=excluded.project_id,title=excluded.title,description=excluded.description,url=excluded.url,due_date=excluded.due_date,completed=excluded.completed,updated_at=excluded.updated_at,completed_at=COALESCE(excluded.completed_at,tasks.completed_at)",params![t.id,t.area_id,t.project_id,t.title,t.description,t.url,t.due_date,t.completed,t.created_at,t.updated_at,t.completed_at]).map_err(err)?;
                self.save_materials("task_materials","task_id",t.id,None,&t.materials)?;
                self.conn.execute("DELETE FROM subtasks WHERE task_id=?1",[t.id]).map_err(err)?;
                for (pos,s) in t.subtasks.iter().enumerate(){let pos=pos as i64;self.conn.execute("INSERT INTO subtasks(task_id,position,title,completed) VALUES(?1,?2,?3,?4)",params![t.id,pos,s.title,s.completed]).map_err(err)?;self.conn.execute("INSERT INTO subtask_keys(task_id,subtask_position,id) VALUES(?1,?2,?3)",params![t.id,pos,s.id]).map_err(err)?;self.save_materials("subtask_materials","task_id",t.id,Some(pos),&s.materials)?;}
                for table in ["task_rich_descriptions","task_order","task_trash","task_kanban_stage"]{self.conn.execute(&format!("DELETE FROM {table} WHERE task_id=?1"),[t.id]).map_err(err)?;}
                if let Some(doc)=&t.description_doc{self.conn.execute("INSERT INTO task_rich_descriptions(task_id,content) VALUES(?1,?2)",params![t.id,doc.to_string()]).map_err(err)?;}
                if let Some(pos)=t.sort_order{self.conn.execute("INSERT INTO task_order(task_id,position) VALUES(?1,?2)",params![t.id,pos]).map_err(err)?;}
                if let Some(date)=&t.deleted_at{self.conn.execute("INSERT INTO task_trash(task_id,deleted_at) VALUES(?1,?2)",params![t.id,date]).map_err(err)?;}
                if let Some(stage)=stages.get(&t.id.to_string()){self.conn.execute("INSERT INTO task_kanban_stage(task_id,stage) VALUES(?1,?2)",params![t.id,stage]).map_err(err)?;}
            }
            for (table,keep) in [("tasks",&task_ids),("projects",&project_ids),("areas",&area_ids)]{
                let mut q=self.conn.prepare(&format!("SELECT id FROM {table}")).map_err(err)?;let ids=q.query_map([],|r|r.get::<_,i64>(0)).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;drop(q);
                for id in ids{if !keep.contains(&id){self.conn.execute(&format!("DELETE FROM {table} WHERE id=?1"),[id]).map_err(err)?;}}
            }
            Ok(Value::Null)
        }).map(|_|())
    }
    pub fn invoke(&mut self,command:&str,a:Value)->Result<Value>{match command {
        "snapshot"=>self.snapshot(),
        "load_theme"=>Ok(json!(self.setting("theme")?.unwrap_or_else(||"light".into()))),
        "save_theme"=>{let s=a["theme"].as_str().ok_or("Нет темы")?;if !["light","dark"].contains(&s){return Err("Неизвестная тема приложения".into());}self.set_setting("theme",s)?;Ok(Value::Null)},
        "data_path"=>Ok(json!(self.data_dir.join("planner.sqlite").to_string_lossy())),
        "backup"=>self.backup(),
        "save_task"=>{let t:Task=serde_json::from_value(a["task"].clone()).map_err(err)?;self.save_task(&t)},
        "save_area"=>{let name=title(a["name"].as_str().ok_or("Нет названия")?)?.to_owned();let icon=a["icon"].as_str().unwrap_or("circle").to_owned();let color=a["color"].as_str().unwrap_or("#61856c").to_owned();if color.len()!=7||!color.starts_with('#')||!color[1..].chars().all(|c|c.is_ascii_hexdigit()){return Err("Некорректный цвет".into());}self.transaction(||{let maybe_id=a["id"].as_i64();let id=if maybe_id.is_none(){let i=self.alloc("areas")?;self.conn.execute("INSERT INTO areas(id,name,icon,color) VALUES(?1,?2,?3,?4)",params![i,name,icon,color]).map_err(err)?;i}else{let id=maybe_id.unwrap();if !self.exists("areas",id)?{return Err("Запись уже удалена. Обновите список.".into());}self.conn.execute("UPDATE areas SET name=?1,icon=?2,color=?3 WHERE id=?4",params![name,icon,color,id]).map_err(err)?;id};Ok(json!(id))})},
        "save_project"=>{let name=title(a["name"].as_str().ok_or("Нет названия")?)?.to_owned();let area=a["areaId"].as_i64().ok_or("Нет сферы")?;let ms:Vec<Material>=serde_json::from_value(a.get("materials").cloned().unwrap_or(json!([]))).map_err(err)?;self.transaction(||{let maybe_id=a["id"].as_i64();let id=if maybe_id.is_none(){let i=self.alloc("projects")?;self.conn.execute("INSERT INTO projects(id,area_id,name) VALUES(?1,?2,?3)",params![i,area,name]).map_err(err)?;i}else{let id=maybe_id.unwrap();let changed=self.conn.execute("UPDATE projects SET name=?1 WHERE id=?2 AND area_id=?3",params![name,id,area]).map_err(err)?;if changed==0{return Err("Запись уже удалена. Обновите список.".into());}id};self.save_materials("project_materials","project_id",id,None,&ms)?;Ok(json!(id))})},
        "save_area_materials"=>{let id=id_arg(&a)?;let ms:Vec<Material>=serde_json::from_value(a["materials"].clone()).map_err(err)?;self.transaction(||{if !self.exists("areas",id)?{return Err("Сфера не найдена".into());}self.save_materials("area_materials","area_id",id,None,&ms)?;Ok(Value::Null)})},
        "toggle_task"=>{let id=id_arg(&a)?;self.active(id)?;let done=a["completed"].as_bool().ok_or("Нет признака выполнения")?;self.conn.execute("UPDATE tasks SET completed=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?2",params![done,id]).map_err(err)?;Ok(Value::Null)},
        "reschedule_task"=>{let id=id_arg(&a)?;let d=a["dueDate"].as_str();due(d)?;self.active(id)?;self.conn.execute("UPDATE tasks SET due_date=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?2",params![d,id]).map_err(err)?;Ok(Value::Null)},
        "set_board_status"=>{let id=id_arg(&a)?;let status=a["status"].as_str().ok_or("Нет колонки")?;if !["todo","progress","review","done"].contains(&status){return Err("Неизвестная колонка доски".into());}self.active(id)?;self.transaction(||{self.conn.execute("UPDATE tasks SET completed=?1,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?2",params![status=="done",id]).map_err(err)?;self.conn.execute("DELETE FROM task_kanban_stage WHERE task_id=?1",[id]).map_err(err)?;if ["progress","review"].contains(&status){self.conn.execute("INSERT INTO task_kanban_stage(task_id,stage) VALUES(?1,?2)",params![id,status]).map_err(err)?;}Ok(Value::Null)})},
        "reorder_tasks"=>{let ids:Vec<i64>=serde_json::from_value(a["ids"].clone()).map_err(err)?;if ids.iter().collect::<HashSet<_>>().len()!=ids.len(){return Err("Список задач изменился. Повторите перемещение.".into());}self.transaction(||{
            for id in &ids{self.active(*id).map_err(|_|"Список задач изменился. Повторите перемещение.".to_string())?;}
            let mut q=self.conn.prepare("SELECT t.id FROM tasks t LEFT JOIN task_order o ON o.task_id=t.id ORDER BY COALESCE(o.position,0),t.created_at DESC,t.id DESC").map_err(err)?;
            let current=q.query_map([],|r|r.get::<_,i64>(0)).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;drop(q);
            let selected=ids.iter().copied().collect::<HashSet<_>>();let mut requested=ids.iter();
            for (p,current_id) in current.iter().enumerate(){let id=if selected.contains(current_id){*requested.next().ok_or("Список задач изменился. Повторите перемещение.")?}else{*current_id};
                self.conn.execute("INSERT INTO task_order(task_id,position) VALUES(?1,?2) ON CONFLICT(task_id) DO UPDATE SET position=excluded.position",params![id,p as i64]).map_err(err)?;}
            Ok(Value::Null)})},
        "delete_task"=>{let id=id_arg(&a)?;let n=self.conn.execute("INSERT INTO task_trash(task_id) SELECT id FROM tasks WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM task_trash WHERE task_id=?1)",[id]).map_err(err)?;if n==0{return Err("Запись уже удалена. Обновите список.".into());}Ok(Value::Null)},
        "restore_task"=>{let id=id_arg(&a)?;let n=self.conn.execute("DELETE FROM task_trash WHERE task_id=?1",[id]).map_err(err)?;if n==0{return Err("Запись уже удалена. Обновите список.".into());}Ok(Value::Null)},
        "purge_task"=>{let id=id_arg(&a)?;let n=self.conn.execute("DELETE FROM tasks WHERE id=?1 AND EXISTS(SELECT 1 FROM task_trash WHERE task_id=?1)",[id]).map_err(err)?;if n==0{return Err("Запись уже удалена. Обновите список.".into());}Ok(Value::Null)},
        "empty_trash"=>{self.conn.execute("DELETE FROM tasks WHERE id IN(SELECT task_id FROM task_trash)",[]).map_err(err)?;Ok(Value::Null)},
        "delete_project"=>{let id=id_arg(&a)?;self.transaction(||{if !self.exists("projects",id)?{return Err("Запись уже удалена. Обновите список.".into());}if a["deleteTasks"].as_bool().unwrap_or(false){self.conn.execute("INSERT OR IGNORE INTO task_trash(task_id) SELECT id FROM tasks WHERE project_id=?1",[id]).map_err(err)?;}self.conn.execute("UPDATE tasks SET project_id=NULL,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE project_id=?1",[id]).map_err(err)?;self.conn.execute("DELETE FROM projects WHERE id=?1",[id]).map_err(err)?;Ok(Value::Null)})},
        "delete_area"=>{let id=id_arg(&a)?;self.transaction(||{if !self.exists("areas",id)?{return Err("Запись уже удалена. Обновите список.".into());}self.conn.execute("UPDATE tasks SET area_id=NULL,project_id=NULL,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE area_id=?1",[id]).map_err(err)?;self.conn.execute("DELETE FROM projects WHERE area_id=?1",[id]).map_err(err)?;self.conn.execute("DELETE FROM areas WHERE id=?1",[id]).map_err(err)?;Ok(Value::Null)})},
        "telegram_source_task_ids"=>{let mut q=self.conn.prepare("SELECT task_id FROM telegram_task_sources").map_err(err)?;let ids=q.query_map([],|r|r.get::<_,i64>(0)).map_err(err)?.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;Ok(json!(ids))},
        "telegram_candidates"=>self.telegram_candidates(),
        "telegram_dismiss"=>{self.conn.execute("UPDATE telegram_candidates SET status='dismissed' WHERE chat_id=?1 AND message_id=?2 AND status='pending'",params![a["chatId"].as_str().ok_or("Нет чата")?,a["messageId"].as_i64().ok_or("Нет сообщения")?]).map_err(err)?;Ok(Value::Null)},
        "telegram_accept"=>self.telegram_accept(&a),
        _=>Err(format!("Команда не относится к локальному хранилищу: {command}")),
    }}
    fn telegram_candidates(&self)->Result<Value>{let mut q=self.conn.prepare("SELECT chat_id,message_id,chat_name,sender,body,sent_at,confidence,bucket FROM telegram_candidates WHERE status='pending' ORDER BY sent_at DESC LIMIT 200").map_err(err)?;let rows=q.query_map([],|r|{let body:String=r.get(4)?;let suggested=body.lines().next().unwrap_or("Задача из Telegram").chars().take(300).collect::<String>();Ok(json!({"chat_id":r.get::<_,String>(0)?,"message_id":r.get::<_,i64>(1)?,"chat_name":r.get::<_,String>(2)?,"sender":r.get::<_,String>(3)?,"text":body,"sent_at":r.get::<_,String>(5)?,"confidence":r.get::<_,f64>(6)?,"bucket":r.get::<_,String>(7)?,"suggested_title":suggested}))}).map_err(err)?;Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?))}
    fn telegram_accept(&self,a:&Value)->Result<Value>{let chat=a["chatId"].as_str().ok_or("Нет чата")?;let message=a["messageId"].as_i64().ok_or("Нет сообщения")?;let name=title(a["title"].as_str().ok_or("Нет названия")?)?;self.transaction(||{let source:Option<(String,String,String,String)>=self.conn.query_row("SELECT chat_name,sender,body,sent_at FROM telegram_candidates WHERE chat_id=?1 AND message_id=?2 AND status='pending'",params![chat,message],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(err)?;let (chat_name,sender,body,date)=source.ok_or("Сообщение уже обработано")?;let id=self.alloc("tasks")?;self.conn.execute("INSERT INTO tasks(id,title,description) VALUES(?1,?2,?3)",params![id,name,format!("Из Telegram · {chat_name} · {sender} · {date}\n\n{body}")]).map_err(err)?;self.conn.execute("UPDATE telegram_candidates SET status='accepted',task_id=?3 WHERE chat_id=?1 AND message_id=?2",params![chat,message,id]).map_err(err)?;self.conn.execute("INSERT INTO telegram_task_sources(task_id,chat_id,message_id) VALUES(?1,?2,?3)",params![id,chat,message]).map_err(err)?;Ok(json!(id))})}
    fn backup(&self)->Result<Value>{
        let stamp=chrono::Local::now().format("%Y%m%d-%H%M%S-%f").to_string();let dir=self.data_dir.join("backups").join(format!("sfery-{stamp}"));fs::create_dir_all(&dir).map_err(err)?;
        // VACUUM INTO creates a consistent SQLite snapshot including committed WAL pages.
        let target=dir.join("planner.sqlite");self.conn.execute("VACUUM INTO ?1",[target.to_string_lossy().as_ref()]).map_err(err)?;
        let dest=dir.join("attachments");fs::create_dir_all(&dest).map_err(err)?;for entry in fs::read_dir(self.attachments_dir()).map_err(err)?{let entry=entry.map_err(err)?;if entry.file_type().map_err(err)?.is_file(){fs::copy(entry.path(),dest.join(entry.file_name())).map_err(err)?;}}
        Ok(json!(dir.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture{store:Store,dir:PathBuf}
    impl Fixture{fn new()->Self{let dir=std::env::temp_dir().join(format!("sfery-recovered-test-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));Self{store:Store::new(&dir).unwrap(),dir}}}
    impl Drop for Fixture{fn drop(&mut self){let _=self.store.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");if let Ok(memory)=Connection::open_in_memory(){let old=std::mem::replace(&mut self.store.conn,memory);drop(old);}if self.dir.starts_with(std::env::temp_dir())&&self.dir.file_name().is_some_and(|n|n.to_string_lossy().starts_with("sfery-recovered-test-")){let _=fs::remove_dir_all(&self.dir);}}}
    fn call(s:&mut Store,c:&str,a:Value)->Value{s.invoke(c,a).unwrap()}
    #[test]fn task_roundtrip_trash_restore_purge_id_not_reused(){let mut f=Fixture::new();let s=&mut f.store;let first=call(s,"save_task",json!({"task":{"title":"Первая","subtasks":[{"id":"s1","title":"Подзадача"}],"description_doc":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Описание"}]}]}}})).as_i64().unwrap();assert_eq!(s.snapshot().unwrap()["tasks"][0]["subtasks"][0]["id"],"s1");call(s,"toggle_task",json!({"id":first,"completed":true}));assert!(s.snapshot().unwrap()["tasks"][0]["completed_at"].is_string());call(s,"set_board_status",json!({"id":first,"status":"review"}));let v=s.snapshot().unwrap();assert_eq!(v["tasks"][0]["completed"],false);assert_eq!(v["kanban_stages"][first.to_string()],"review");call(s,"delete_task",json!({"id":first}));assert_eq!(s.snapshot().unwrap()["trash"].as_array().unwrap().len(),1);assert!(s.invoke("toggle_task",json!({"id":first,"completed":true})).is_err());call(s,"restore_task",json!({"id":first}));assert_eq!(s.snapshot().unwrap()["tasks"].as_array().unwrap().len(),1);assert!(s.invoke("purge_task",json!({"id":first})).is_err());assert_eq!(s.snapshot().unwrap()["tasks"].as_array().unwrap().len(),1);call(s,"delete_task",json!({"id":first}));call(s,"purge_task",json!({"id":first}));let second=call(s,"save_task",json!({"task":{"title":"Вторая"}})).as_i64().unwrap();assert_ne!(second,first);assert!(second>0&&second<=9_007_199_254_740_991);}
    #[test]fn project_area_delete_preserves_task_and_materials_transaction(){let mut f=Fixture::new();let s=&mut f.store;let area=call(s,"save_area",json!({"name":"Сфера","color":"#123abc"}));let project=call(s,"save_project",json!({"name":"Проект","areaId":area,"materials":[{"id":"link1","name":"Ссылка","kind":"link","url":"https://example.com"}]}));let task=call(s,"save_task",json!({"task":{"title":"Задача","area_id":area,"project_id":project}}));assert_eq!(s.snapshot().unwrap()["projects"][0]["materials"][0]["id"],"link1");assert!(s.invoke("save_project",json!({"id":project,"name":"Не сохранять","areaId":area,"materials":[{"id":"missing","name":"Файл","kind":"file","size":5}]})).is_err());assert_eq!(s.snapshot().unwrap()["projects"][0]["name"],"Проект");call(s,"delete_project",json!({"id":project,"deleteTasks":true}));assert_eq!(s.snapshot().unwrap()["trash"][0]["project_id"],Value::Null);call(s,"restore_task",json!({"id":task}));call(s,"delete_area",json!({"id":area}));assert_eq!(s.snapshot().unwrap()["tasks"][0]["area_id"],Value::Null);}
    #[test]fn ordering_validation_and_reopen(){let mut f=Fixture::new();let s=&mut f.store;let a=call(s,"save_task",json!({"task":{"title":"A"}}));let b=call(s,"save_task",json!({"task":{"title":"B"}}));call(s,"reorder_tasks",json!({"ids":[a,b]}));assert_eq!(s.snapshot().unwrap()["tasks"][0]["title"],"A");assert!(s.invoke("reorder_tasks",json!({"ids":[a,a]})).is_err());assert!(s.invoke("save_task",json!({"task":{"title":"  "}})).is_err());assert!(s.invoke("reschedule_task",json!({"id":a,"dueDate":"2026-02-30"})).is_err());let reopened=Store::new(&f.dir).unwrap();assert_eq!(reopened.snapshot().unwrap()["tasks"].as_array().unwrap().len(),2);}
    #[test]fn backup_is_readable_snapshot(){let mut f=Fixture::new();call(&mut f.store,"save_task",json!({"task":{"title":"Сохранить"}}));let path=call(&mut f.store,"backup",json!({}));let c=Connection::open(Path::new(path.as_str().unwrap()).join("planner.sqlite")).unwrap();assert_eq!(c.query_row("SELECT title FROM tasks",[],|r|r.get::<_,String>(0)).unwrap(),"Сохранить");}
    #[test]fn file_material_path_is_owned_and_metadata_verified(){let mut f=Fixture::new();let s=&mut f.store;s.conn.execute("INSERT INTO file_assets(id,name,kind,size) VALUES('file1','image.png','image',3)",[]).unwrap();fs::write(s.attachments_dir().join("file1"),b"abc").unwrap();let id=call(s,"save_task",json!({"task":{"title":"С материалом","materials":[{"id":"file1","name":"image.png","kind":"image","size":3,"path":"C:/external/untrusted"}]}}));let snap=s.snapshot().unwrap();assert_eq!(snap["tasks"][0]["materials"][0]["path"],s.attachments_dir().join("file1").to_string_lossy().as_ref());assert!(s.invoke("save_task",json!({"task":{"id":id,"title":"Не менять","materials":[{"id":"file1","name":"image.png","kind":"image","size":4}]}})).is_err());assert_eq!(s.snapshot().unwrap()["tasks"][0]["title"],"С материалом");}
    #[test]fn telegram_candidate_accept_is_transactional_and_keeps_provenance(){let mut f=Fixture::new();let s=&mut f.store;s.conn.execute("INSERT INTO telegram_candidates(chat_id,message_id,chat_name,sender,body,sent_at) VALUES('chat',10,'Чат','Автор','Текст сообщения','2026-10-16T00:00:00Z')",[]).unwrap();assert_eq!(call(s,"telegram_candidates",json!({}))[0]["text"],"Текст сообщения");let id=call(s,"telegram_accept",json!({"chatId":"chat","messageId":10,"title":"Создано"}));assert_eq!(call(s,"telegram_source_task_ids",json!({})),json!([id]));assert!(s.invoke("telegram_accept",json!({"chatId":"chat","messageId":10,"title":"Дважды"})).is_err());assert_eq!(s.snapshot().unwrap()["tasks"].as_array().unwrap().len(),1);assert_eq!(call(s,"telegram_candidates",json!({})),json!([]));}
    #[test]fn original_synthetic_fixture_parity_and_snapshot_roundtrip(){let mut f=Fixture::new();let memory=Connection::open_in_memory().unwrap();let old=std::mem::replace(&mut f.store.conn,memory);drop(old);fs::write(f.dir.join("planner.sqlite"),include_bytes!("../fixtures/original-probe.sqlite")).unwrap();f.store=Store::new(&f.dir).unwrap();let before=f.store.snapshot().unwrap();assert_eq!(before["tasks"][0]["id"],json!(8418832499662761_i64));assert_eq!(before["tasks"][0]["subtasks"][0]["id"],"probe-sub");assert_eq!(before["tasks"][0]["materials"][0]["size"],20);assert_eq!(before["projects"][0]["materials"][0]["url"],"https://example.com");f.store.replace_snapshot(&before).unwrap();assert_eq!(before,f.store.snapshot().unwrap());let raw:String=f.store.conn.query_row("SELECT content FROM task_rich_descriptions",[],|r|r.get(0)).unwrap();assert!(raw.contains("Probe description"));}
    #[test]fn replacement_preserves_settings_sources_and_rolls_back_invalid_data(){let mut f=Fixture::new();let s=&mut f.store;let id=call(s,"save_task",json!({"task":{"title":"Исходная"}}));s.conn.execute("INSERT INTO telegram_task_sources(task_id,chat_id,message_id) VALUES(?1,'chat',10)",[id.as_i64().unwrap()]).unwrap();s.set_setting("theme","dark").unwrap();let mut next=s.snapshot().unwrap();next["tasks"][0]["title"]=json!("Из облака");s.replace_snapshot(&next).unwrap();assert_eq!(s.setting("theme").unwrap().as_deref(),Some("dark"));assert_eq!(call(s,"telegram_source_task_ids",json!({})),json!([id]));next["tasks"][0]["materials"]=json!([{"id":"unsafe/path","name":"Ошибка","kind":"file","size":1}]);assert!(s.replace_snapshot(&next).is_err());assert_eq!(s.snapshot().unwrap()["tasks"][0]["title"],"Из облака");}
    #[test]fn additive_migration_preserves_legacy_records(){let mut f=Fixture::new();let memory=Connection::open_in_memory().unwrap();drop(std::mem::replace(&mut f.store.conn,memory));fs::remove_file(f.dir.join("planner.sqlite")).unwrap();let old=Connection::open(f.dir.join("planner.sqlite")).unwrap();old.execute_batch(include_str!("../fixtures/legacy-schema.sql")).unwrap();old.execute("INSERT INTO tasks(id,title,completed) VALUES(123,'Старая выполненная задача',1)",[]).unwrap();old.execute("INSERT INTO telegram_candidates(chat_id,message_id,chat_name,sender,body,sent_at) VALUES('chat',10,'Чат','Автор','Текст','2026-10-16')",[]).unwrap();drop(old);f.store=Store::new(&f.dir).unwrap();let snap=f.store.snapshot().unwrap();assert_eq!(snap["tasks"][0]["id"],123);assert!(snap["tasks"][0]["completed_at"].is_string());let candidate=call(&mut f.store,"telegram_candidates",json!({}));assert_eq!(candidate[0]["confidence"],1.0);assert_eq!(candidate[0]["bucket"],"high");}
    #[test]fn native_defaults_and_rejected_operation_semantics(){let mut f=Fixture::new();let s=&mut f.store;let id=call(s,"save_task",json!({"task":{"title":"Новая","subtasks":[{"title":"Шаг","completed":false}]}}));let snap=s.snapshot().unwrap();assert_eq!(snap["tasks"][0]["due_date"],chrono::Local::now().format("%Y-%m-%d").to_string());assert_eq!(snap["tasks"][0]["subtasks"][0]["id"].as_str().unwrap().len(),36);assert_eq!(s.invoke("purge_task",json!({"id":id})).unwrap_err(),"Запись уже удалена. Обновите список.");assert_eq!(s.invoke("delete_area",json!({"id":987654321})).unwrap_err(),"Запись уже удалена. Обновите список.");assert_eq!(s.invoke("reschedule_task",json!({"id":987654321,"dueDate":"2026-02-30"})).unwrap_err(),"Некорректная дата");assert_eq!(s.invoke("save_task",json!({"task":{"id":-1,"title":"Не создавать"}})).unwrap_err(),"Запись уже удалена. Обновите список.");}
    #[test]fn native_fk_errors_and_link_path_null(){let mut f=Fixture::new();let s=&mut f.store;assert_eq!(s.invoke("save_task",json!({"task":{"title":"Не сохранять","area_id":987654321}})).unwrap_err(),"Не удалось сохранить или прочитать данные: FOREIGN KEY constraint failed");call(s,"save_task",json!({"task":{"title":"Со ссылкой","materials":[{"id":"link","name":"Ссылка","kind":"link","url":"https://example.com","size":0}]}}));assert!(s.snapshot().unwrap()["tasks"][0]["materials"][0].get("path").unwrap().is_null());}
    #[test]fn native_reorder_replaces_selected_slots_and_renumbers_trash(){let mut f=Fixture::new();let s=&mut f.store;let a=call(s,"save_task",json!({"task":{"title":"A"}}));let b=call(s,"save_task",json!({"task":{"title":"B"}}));let trash=call(s,"save_task",json!({"task":{"title":"Trashed"}}));call(s,"delete_task",json!({"id":trash}));call(s,"reorder_tasks",json!({"ids":[]}));let snap=s.snapshot().unwrap();assert_eq!(snap["trash"][0]["sort_order"],0);assert_eq!(snap["tasks"][0]["title"],"B");assert_eq!(snap["tasks"][0]["sort_order"],1);assert_eq!(snap["tasks"][1]["sort_order"],2);call(s,"reorder_tasks",json!({"ids":[a]}));assert_eq!(s.snapshot().unwrap()["tasks"][1]["sort_order"],2);call(s,"reorder_tasks",json!({"ids":[a,b]}));assert_eq!(s.snapshot().unwrap()["tasks"][0]["title"],"A");let before=s.snapshot().unwrap();assert!(s.invoke("reorder_tasks",json!({"ids":[trash]})).is_err());assert_eq!(before,s.snapshot().unwrap());}
    #[test]fn native_task_update_null_due_preserves_completion_and_subtask_identity(){let mut f=Fixture::new();let s=&mut f.store;let id=call(s,"save_task",json!({"task":{"title":"Roundtrip","due_date":"2026-12-01","subtasks":[{"id":"A","title":"A","completed":false},{"id":"B","title":"B","completed":true}]}}));call(s,"toggle_task",json!({"id":id,"completed":true}));let mut task=s.snapshot().unwrap()["tasks"][0].clone();let completed=task["completed_at"].clone();task["description"]=json!("Changed");task["due_date"]=Value::Null;task["subtasks"].as_array_mut().unwrap().reverse();call(s,"save_task",json!({"task":task}));let now=s.snapshot().unwrap();assert_eq!(now["tasks"][0]["completed_at"],completed);assert!(now["tasks"][0]["due_date"].is_null());assert_eq!(now["tasks"][0]["subtasks"][0]["id"],"B");assert_eq!(now["tasks"][0]["subtasks"][1]["id"],"A");}
    #[test]fn exact_native_reservation_and_high_water_triggers_survive_deletion(){let mut f=Fixture::new();let s=&mut f.store;s.conn.execute("INSERT INTO tasks(id,title) VALUES(900,'High')",[]).unwrap();s.conn.execute("INSERT INTO tasks(id,title) VALUES(100,'Low')",[]).unwrap();assert_eq!(s.setting("id_high_water_tasks").unwrap().as_deref(),Some("900"));s.conn.execute("DELETE FROM tasks WHERE id=900",[]).unwrap();assert!(s.conn.query_row("SELECT EXISTS(SELECT 1 FROM allocated_ids WHERE kind='tasks' AND id=900)",[],|r|r.get::<_,bool>(0)).unwrap());assert_eq!(s.setting("id_high_water_tasks").unwrap().as_deref(),Some("900"));let names=s.conn.prepare("SELECT name FROM sqlite_schema WHERE type='trigger'").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();assert!(names.contains(&"tasks_reserve_id".to_string()));assert!(names.contains(&"tasks_id_high_water".to_string()));}
}
