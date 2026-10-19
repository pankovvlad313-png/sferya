const fs=require('node:fs');
const path=require('node:path');
const {DatabaseSync}=require('node:sqlite');
if(process.env.GITHUB_ACTIONS!=='true')throw Error('Only seed the disposable GitHub Actions profile');
const dir=path.join(process.env.APPDATA,'local.sfery.planner');
if(fs.existsSync(path.join(dir,'planner.sqlite')))throw Error('Refusing to overwrite an existing database');
fs.mkdirSync(path.join(dir,'attachments'),{recursive:true});
fs.copyFileSync(path.join(__dirname,'../src-tauri/fixtures/original-probe.sqlite'),path.join(dir,'planner.sqlite'));
const db=new DatabaseSync(path.join(dir,'planner.sqlite'),{readOnly:true});
for(const file of db.prepare('SELECT id,size FROM file_assets').all()){
  if(!/^[a-zA-Z0-9._-]+$/.test(file.id)||file.size>10000)throw Error('Fixture is not synthetic');
  fs.writeFileSync(path.join(dir,'attachments',file.id),Buffer.alloc(file.size,65));
}
db.close();
