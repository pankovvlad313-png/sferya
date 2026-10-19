//! IPC argument shapes measured against the isolated original native application.
//! Internal Store helpers intentionally retain convenient defaults for tests/imports.
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename = "Material")]
struct NativeMaterial {
    id:String, name:String, kind:String, url:String, size:u64, path:Option<String>,
}
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename = "Subtask")]
struct NativeSubtask {
    #[serde(default)] id:String,
    title:String, completed:bool,
    #[serde(default)] materials:Vec<NativeMaterial>,
}
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename = "Task")]
struct NativeTask {
    id:i64, area_id:Option<i64>, project_id:Option<i64>, title:String,
    description:String, description_doc:Option<Value>, url:String,
    due_date:Option<String>, completed:bool, created_at:String, updated_at:String,
    completed_at:Option<String>,
    #[serde(default)] subtasks:Vec<NativeSubtask>,
    #[serde(default)] materials:Vec<NativeMaterial>,
    sort_order:Option<i64>, deleted_at:Option<String>,
}
fn arg<T:DeserializeOwned>(command:&str,args:&Value,key:&str,optional:bool)->Result<T,String>{
    let value=match args.get(key){Some(v)=>v.clone(),None if optional=>Value::Null,None=>return Err(format!("invalid args `{key}` for command `{command}`: command {command} missing required key {key}"))};
    serde_json::from_value(value).map_err(|e|format!("invalid args `{key}` for command `{command}`: {e}"))
}
pub fn arguments(command:&str,mut args:Value)->Result<Value,String>{
    match command {
        "save_task"=>{arg::<NativeTask>(command,&args,"task",false)?;},
        "save_area"=>{arg::<Option<i64>>(command,&args,"id",true)?;for key in ["name","icon","color"]{arg::<String>(command,&args,key,false)?;}},
        "save_project"=>{arg::<Option<i64>>(command,&args,"id",true)?;arg::<i64>(command,&args,"areaId",false)?;arg::<String>(command,&args,"name",false)?;arg::<Option<Vec<NativeMaterial>>>(command,&args,"materials",true)?;if args.get("materials").is_none_or(Value::is_null){args["materials"]=serde_json::json!([]);}},
        "save_area_materials"=>{arg::<i64>(command,&args,"id",false)?;arg::<Vec<NativeMaterial>>(command,&args,"materials",false)?;},
        "toggle_task"=>{arg::<i64>(command,&args,"id",false)?;arg::<bool>(command,&args,"completed",false)?;},
        "set_board_status"=>{arg::<i64>(command,&args,"id",false)?;arg::<String>(command,&args,"status",false)?;},
        "reschedule_task"=>{arg::<i64>(command,&args,"id",false)?;arg::<Option<String>>(command,&args,"dueDate",true)?;},
        "delete_project"=>{arg::<i64>(command,&args,"id",false)?;arg::<bool>(command,&args,"deleteTasks",false)?;},
        "delete_area"|"delete_task"|"restore_task"|"purge_task"=>{arg::<i64>(command,&args,"id",false)?;},
        "reorder_tasks"=>{arg::<Vec<i64>>(command,&args,"ids",false)?;},
        "save_theme"=>{arg::<String>(command,&args,"theme",false)?;},
        "finish_attachment"|"read_text_attachment"|"open_attachment"|"show_attachment"=>{arg::<String>(command,&args,"id",false)?;},
        "open_url"=>{arg::<String>(command,&args,"url",false)?;},
        _=>{},
    }Ok(args)
}
pub fn response(command:&str,value:Value)->Value{
    if matches!(command,"save_area"|"save_project"|"save_task"|"telegram_accept"){Value::Null}else{value}
}

#[cfg(test)]
mod tests{
    use super::*;use serde_json::json;
    #[test]fn original_task_missing_fields_are_rejected_before_storage(){assert_eq!(arguments("save_task",json!({"task":{"id":0,"title":"Test"}})).unwrap_err(),"invalid args `task` for command `save_task`: missing field `description`");}
    #[test]fn original_scalar_types_and_required_argument_messages(){assert_eq!(arguments("save_area",json!({})).unwrap_err(),"invalid args `name` for command `save_area`: command save_area missing required key name");assert_eq!(arguments("reorder_tasks",json!({"ids":[1,null]})).unwrap_err(),"invalid args `ids` for command `reorder_tasks`: invalid type: null, expected i64");}
    #[test]fn optional_project_materials_and_native_mutation_response(){assert_eq!(arguments("save_project",json!({"name":"Test","areaId":1})).unwrap()["materials"],json!([]));assert_eq!(response("save_task",json!(123)),Value::Null);assert_eq!(response("snapshot",json!({})),json!({}));}
    #[test]fn task_option_fields_and_defaulted_vectors_are_accepted(){let task=json!({"id":0,"title":"Test","description":"","url":"","completed":false,"created_at":"","updated_at":""});assert!(arguments("save_task",json!({"task":task})).is_ok());}
}
