const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto');
const {DatabaseSync}=require('node:sqlite');
if(process.env.GITHUB_ACTIONS!=='true')throw Error('Only use the disposable Actions profile');
const [mode,output]=process.argv.slice(2);
const dir=path.join(process.env.APPDATA,'local.sfery.planner');
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const db=new DatabaseSync(path.join(dir,'planner.sqlite'),{readOnly:true});
const tables=Object.fromEntries(db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'").all().map(({name})=>[name,db.prepare(`SELECT * FROM "${name.replaceAll('"','""')}"`).all().map(row=>sha(JSON.stringify(row))).sort()]));
db.close();
const files={};
function inventory(folder){for(const e of fs.readdirSync(folder,{withFileTypes:true})){const p=path.join(folder,e.name);if(e.isSymbolicLink())throw Error('Fixture contains a symlink');if(e.isDirectory())inventory(p);else files[path.relative(dir,p)]=sha(fs.readFileSync(p))}}
inventory(path.join(dir,'attachments'));
const baseline=path.join(output,'original-data-baseline.json');
if(mode==='before'){fs.writeFileSync(baseline,JSON.stringify({tables,files},null,2));console.log('Original installed profile baseline captured');}
else if(mode==='after'){
 const before=JSON.parse(fs.readFileSync(baseline));
 for(const [name,rows]of Object.entries(before.tables)){
   if(['app_settings','allocated_ids'].includes(name)){for(const row of rows)assert.ok(tables[name].includes(row),'Original metadata changed: '+name)}
   else assert.deepEqual(tables[name],rows,'Original user records changed: '+name);
 }
 assert.deepEqual(files,before.files,'Attachments changed');
 console.log('All original table rows and attachment bytes preserved');
}else throw Error('Expected before or after mode');
