// Windows integration test. Only the test build and two generated JSON files are opened.
// Build app-new first, then run: node tests/test-profile-recovery.js
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const runs = path.join(root, 'test-runs');
const sleep = ms => new Promise(r => setTimeout(r,ms));

// String normally drives its full smoke suite. Substitute only its test entry
// point in the disposable copy; the application and its startup code stay intact.
function testPage() {
  async function probe() {
    const ok = await N.boot;
    const dir = (await belt.native.paths()).data;
    if (!ok) {
      const button = document.querySelector('dialog[open] button');
      if (!button) throw Error('Recovery prompt missing');
      N.player.setVolume(.33);
      await belt.native.writeText(dir + '/ready.txt','ready');
      while (!(await belt.native.stat(dir + '/unlocked.txt')).exists) await new Promise(r=>setTimeout(r,50));
      if ((await belt.native.stat(dir + '/retry.txt')).exists) {
        sessionStorage.setItem('test-retry','1');
        button.click();
        return;
      }
    } else {
      if (N.S.playlists[0]?.id !== 'fixture' || N.S.liked.original !== 1) throw Error('Original library missing');
      N.player.setVolume(.51);
      N.stats.add({id:'other',T:'Sample'},1);
    }
    window.smokeState={step:'done',done:true,ok};
  }
  window.smokeTest=()=>probe().catch(e=>{window.smokeState={step:'error',done:true,error:e.message};});
  if (sessionStorage.getItem('test-retry')) window.smokeTest();
}

async function main() {
  fs.mkdirSync(runs,{recursive:true});
  const scratch=fs.mkdtempSync(path.join(runs,'recovery-')), app=path.join(scratch,'app');
  fs.mkdirSync(app);
  const built=path.join(root,'app-new');
  for (const f of fs.readdirSync(built,{withFileTypes:true})) {
    if (f.isFile() || ['runtimes','wwwroot'].includes(f.name))
      fs.cpSync(path.join(built,f.name),path.join(app,f.name),{recursive:true});
  }
  fs.writeFileSync(path.join(app,'wwwroot','selftest.js'),'(' + testPage.toString() + ')();');
  for (const [locked,retry] of [['state',false],['listening',false],['state',true]]) {
    const data=fs.mkdtempSync(path.join(scratch,'profile-')), out=path.join(data,'out');
    const state=path.join(data,'state.json'), stats=path.join(data,'listening.json');
    const stateText=JSON.stringify({v:1,playlists:[{id:'fixture',name:'Fixture',items:[]}],liked:{original:1}});
    const statsText=JSON.stringify({v:1,tracks:[],artists:[],days:{'2020-01-01':{s:300,h:[300],a:{},t:{}}}});
    fs.writeFileSync(state,stateText);fs.writeFileSync(stats,statsText);
    if(retry)fs.writeFileSync(path.join(data,'retry.txt'),'retry');
    const lock=spawn('powershell.exe',['-NoProfile','-NonInteractive','-Command',
      "$ErrorActionPreference='Stop'; $f=[IO.File]::Open($env:STRING_TEST_LOCK,'Open','ReadWrite','None'); try { [Console]::WriteLine('locked'); [Console]::ReadLine() | Out-Null } finally { $f.Dispose() }"],
      {windowsHide:true,env:{...process.env,STRING_TEST_LOCK:locked==='state'?state:stats}});
    const released=once(lock,'exit');
    let player;
    try {
      await Promise.race([once(lock.stdout,'data'),released.then(()=>{throw Error('Could not lock fixture');})]);
      player=spawn(path.join(app,'String.exe'),['--smoke-test','--data',data,'--out',out,'--timeout','35'],{windowsHide:true});
      const done=once(player,'exit');
      const deadline=Date.now()+30000;
      while(!fs.existsSync(path.join(data,'ready.txt')) && Date.now()<deadline) await sleep(100);
      assert(fs.existsSync(path.join(data,'ready.txt')),'app did not report protected startup');
      lock.stdin.end('\n');await released;
      assert.equal(fs.readFileSync(state,'utf8'),stateText);assert.equal(fs.readFileSync(stats,'utf8'),statsText);
      fs.writeFileSync(path.join(data,'unlocked.txt'),'unlocked');
      const timeout=setTimeout(()=>player.kill(),50000);
      try { await done; } finally { clearTimeout(timeout); }
      const final=fs.readFileSync(path.join(out,'results.txt'),'utf8').split(/\r?\n/).find(x=>x.startsWith('final'));
      const result=JSON.parse(final.slice(5).trim());
      assert.equal(result.step,'done');assert.equal(result.ok,retry);
      if(retry){
        const recovered=JSON.parse(fs.readFileSync(state));
        assert.equal(recovered.vol,.51);assert.equal(recovered.playlists[0].id,'fixture');assert.equal(recovered.liked.original,1);
        assert.equal(JSON.parse(fs.readFileSync(stats)).days['2020-01-01'].s,300);
      }else{
        assert.equal(fs.readFileSync(state,'utf8'),stateText);assert.equal(fs.readFileSync(stats,'utf8'),statsText);
      }
      console.log('ok   locked '+locked+' file, '+(retry?'Try again restores library and normal saving':'normal close preserves both original files'));
    }finally{
      if(lock.exitCode===null)lock.kill();
      if(player && player.exitCode===null)player.kill();
    }
  }
}
main().catch(e=>{console.error(e);process.exitCode=1;});
