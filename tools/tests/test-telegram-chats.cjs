const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const root=path.resolve(__dirname,'../..');
const desktop=fs.existsSync(path.join(root,'source/desktop'))?path.join(root,'source/desktop'):path.join(root,'desktop');
const bundle=fs.readFileSync(path.join(desktop,'frontend-dist/assets/index-BGD1PF6B-sferik-idle.js'),'utf8');
const fragment=bundle.match(/pe=r\.filter\(oe=>\[oe\.name,oe\.username\?"@"\+oe\.username:""\]\.some\(value=>value\.toLocaleLowerCase\("ru"\)\.includes\(b\.toLocaleLowerCase\("ru"\)\)\)\)/)?.[0];
assert(fragment,'Production chat-search function not found');
const search=Function('r','b','let '+fragment+';return pe;');
const chats=[{id:'9',name:'Мама',username:'anna'},{id:'3',name:'Анна Работа',username:'work'},{id:'7',name:'Папа'}];
test('saved contact names and usernames with or without @ find the same person',()=>{
 for(const query of ['Мама','мама','anna','@anna','@ANNA'])assert.deepEqual(search(chats,query).map(x=>x.id),['9']);
 assert.equal(search(chats,'работа')[0].id,'3');
 assert.equal(search(chats,'папа')[0].id,'7');
});
test('search preserves Telegram order and never rewrites display names',()=>{
 assert.deepEqual(search(chats,'').map(x=>x.id),['9','3','7']);
 assert.equal(search(chats,'anna')[0].name,'Мама');
 assert.deepEqual(search(chats,'absent'),[]);
});
