const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {setTimeout:delay} = require('node:timers/promises');
const [stage, output] = process.argv.slice(2);
if (!process.env.GITHUB_ACTIONS || !['original','candidate'].includes(stage)) throw Error('Only run on the isolated GitHub Actions runner');
const clean = v => {
  if (Array.isArray(v)) return v.map(clean);
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).filter(([k])=>k!=='path').map(([k,x])=>[k,clean(x)]));
  return v;
};
(async()=>{
  let target;
  let lastStatus='No connection';
  for(let i=0;i<80;i++){
    try {const response=await fetch('http://127.0.0.1:9239/json/list');const pages=await response.json();lastStatus=JSON.stringify(pages.map(p=>({type:p.type,url:p.url})));target=pages.find(p=>p.type==='page' && p.webSocketDebuggerUrl);}catch(error){lastStatus=error.cause?.code||error.message}
    if(target)break; await delay(500);
  }
  assert.ok(target,'WebView2 did not start: '+lastStatus);
  const ws=new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject});
  let id=0;const pending=new Map();
  ws.onmessage=e=>{const m=JSON.parse(e.data);const p=pending.get(m.id);if(p){clearTimeout(p.timer);pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}};
  const rpc=(method,params={})=>new Promise((resolve,reject)=>{const n=++id;const timer=setTimeout(()=>reject(Error('CDP timeout: '+method)),30000);pending.set(n,{resolve,reject,timer});ws.send(JSON.stringify({id:n,method,params}))});
  await delay(9000);
  const evaluation=await rpc('Runtime.evaluate',{expression:`(async()=>{const invoke=window.__TAURI_INTERNALS__.invoke;return {snapshot:await invoke('snapshot'),theme:await invoke('load_theme'),dataPath:await invoke('data_path'),title:document.title,text:document.body.innerText}})()`,awaitPromise:true,returnByValue:true});
  assert.ok(!evaluation.exceptionDetails,JSON.stringify(evaluation.exceptionDetails));
  const result=evaluation.result.value;
  assert.match(result.title,/Sfery/);
  assert.ok(/Сегодня|Все задачи|Пользовательское соглашение/.test(result.text),'Expected the planner or its first-launch agreement screen');
  assert.equal(path.resolve(result.dataPath).toLowerCase(),path.resolve(process.env.APPDATA,'local.sfery.planner/planner.sqlite').toLowerCase());
  const baselinePath=path.join(output,'original-baseline.json');
  if(stage==='original')fs.writeFileSync(baselinePath,JSON.stringify({snapshot:clean(result.snapshot),theme:result.theme},null,2));
  else {
    const original=JSON.parse(fs.readFileSync(baselinePath));
    assert.deepEqual(clean(result.snapshot),original.snapshot,'Planner data changed during installed upgrade');
    assert.equal(result.theme,original.theme);
    const {spawn}=require('node:child_process');
    await new Promise((resolve,reject)=>{
      const child=spawn(process.env.SFERY_TEST_EXE,[],{windowsHide:true,stdio:'ignore'});
      const timer=setTimeout(()=>{child.kill();reject(Error('Second instance did not hand over to the first'));},10000);
      child.on('error',error=>{clearTimeout(timer);reject(error)});
      child.on('exit',code=>{clearTimeout(timer);code===0?resolve():reject(Error('Second instance exited with '+code))});
    });
    const stillAlive=await rpc('Runtime.evaluate',{expression:"document.title",returnByValue:true});
    assert.match(stillAlive.result.value,/Sfery/);
  }
  const screen=await rpc('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});
  fs.writeFileSync(path.join(output,`${stage}.png`),Buffer.from(screen.data,'base64'));
  fs.writeFileSync(path.join(output,`${stage}-smoke.json`),JSON.stringify({passed:true,title:result.title,dataPath:result.dataPath,counts:Object.fromEntries(['areas','projects','tasks','trash'].map(k=>[k,result.snapshot[k].length]))},null,2));
  console.log(`${stage}: UI and original data path verified`);
  await rpc('Runtime.evaluate',{expression:`window.__TAURI_INTERNALS__.invoke('plugin:window|close',{label:'main'}).catch(()=>{})`}).catch(()=>{});
  ws.close();
})().catch(e=>{console.error(e);process.exitCode=1});
