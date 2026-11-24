/** Hidden signed-app acceptance for Agent controls; synthetic data only.
 * node docs/testing/agent-production-ui.mjs /private/tmp/.../Interface.app */
import { mkdtemp, mkdir, writeFile, readFile, copyFile, chmod } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { packagedSandboxEnvironment, packagedSandboxPolicy } from '../../electron/scripts/packaged-sandbox.mjs';
const appPath=path.resolve(process.argv[2]);
if (!(await readFile(path.join(appPath,'Contents/Resources/app.asar'))).includes(Buffer.from('iface-packaged-smoke-isolation-v1'))) throw new Error('Current smoke isolation required');
const root=await mkdtemp('/private/tmp/agent-ui-');
for(const name of ['data','home','process-home','tmp','bin']) await mkdir(path.join(root,name),{mode:0o700});
await mkdir(path.join(root,'home/Desktop/Projects'),{recursive:true,mode:0o700});
for(const name of ['claude','codex']) {
 const target=path.join(root,'bin',name);
 await copyFile(fileURLToPath(new URL('./agent-production-fake-provider.py',import.meta.url)),target);await chmod(target,0o700);
}
for(const name of ['grok','qwen','agy','omp','opencode','qwen-mlx-server']) await writeFile(path.join(root,'bin',name),'#!/bin/sh\nexit 64\n',{mode:0o700});
for(const name of ['home','process-home']) await writeFile(path.join(root,name,'.zprofile'),`export PATH=${root}/bin:/usr/bin:/bin:/usr/sbin:/sbin\n`,{mode:0o600});
await writeFile(path.join(root,'host.sb'),packagedSandboxPolicy(root),{mode:0o600});
const server=net.createServer();await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;await new Promise(resolve=>server.close(resolve));
const child=spawn(path.join(appPath,'Contents/MacOS/Interface'),[`--user-data-dir=${root}`,`--remote-debugging-port=${port}`,'--remote-debugging-address=127.0.0.1','--disable-background-timer-throttling','--disable-backgrounding-occluded-windows','--disable-renderer-backgrounding'],{cwd:root,env:{...packagedSandboxEnvironment(root),CLAUDE_CODE_EXECUTABLE:path.join(root,'bin/claude'),CODEX_EXECUTABLE:path.join(root,'bin/codex')},stdio:['ignore','pipe','pipe']});
let logs='';for(const s of [child.stdout,child.stderr])s.on('data',v=>{logs=(logs+v).slice(-32000)});
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));let ws;let seq=0;const pending=new Map();
try {
 let target;
 for(let i=0;i<200;i++) {
  if(child.exitCode!==null)throw new Error(`App exited ${child.exitCode}`);
  try {target=(await(await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(v=>v.type==='page'&&v.url.startsWith('file:'));}catch{}
  if(target)break;await pause(100);
 }
 if(!target)throw new Error('No private renderer target');
 ws=new WebSocket(target.webSocketDebuggerUrl);
 await new Promise((resolve,reject)=>{ws.addEventListener('open',resolve,{once:true});ws.addEventListener('error',reject,{once:true})});
 ws.addEventListener('message',event=>{const r=JSON.parse(event.data);if(!r.id)return;const p=pending.get(r.id);if(!p)return;pending.delete(r.id);clearTimeout(p.timer);r.error?p.reject(new Error(JSON.stringify(r.error))):p.resolve(r.result)});
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`CDP timeout ${method}`))},90000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}))});
 await call('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
 const response=await call('Runtime.evaluate',{awaitPromise:true,returnByValue:true,expression:`(${async function(){
  const results=[];
  const assert=(yes,message)=>{if(!yes)throw new Error(message)};
  const wait=async(read,label)=>{for(let i=0;i<200;i++){const value=await read();if(value)return value;await new Promise(r=>setTimeout(r,50))}throw new Error('Wait: '+label)};
  const button=(text,root=document)=>[...root.querySelectorAll('button')].find(b=>b.textContent.trim()===text&&!b.disabled);
  const click=async(text,root=document)=>(await wait(()=>button(text,root),text)).click();
  const fill=async(el,value)=>{assert(el,'Missing input');const proto=el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:el.tagName==='SELECT'?HTMLSelectElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(el,value);el.dispatchEvent(new Event(el.tagName==='SELECT'?'change':'input',{bubbles:true}));await new Promise(r=>setTimeout(r,30))};
  const field=(text,root=document)=>[...root.querySelectorAll('label')].find(l=>l.firstChild?.textContent.trim()===text)?.querySelector('input,textarea,select');
  const tab=async(text)=>{(await wait(()=>[...document.querySelectorAll('.agent-workspace-nav button')].find(b=>b.textContent.startsWith(text)),text)).click();await new Promise(r=>setTimeout(r,100))};
  await wait(()=>window.iface&&document.querySelector('.workspace-toolbar'),'ready');
  document.querySelector('.surface-switch-tab[aria-label="Agent"]').click();
  (await wait(()=>document.querySelector('button[aria-label="New agent"]:not(:disabled)'),'new agent')).click();
  const dialog=await wait(()=>document.querySelector('.agent-dialog'),'dialog');
  await fill(dialog.querySelector('input.agent-dialog-input'),'Signed UI fixture');
  (await wait(()=>dialog.querySelector('.agent-dialog-submit:not(:disabled)'),'create')).click();
  await wait(()=>document.querySelector('.agent-workspace-nav'),'workspace');
  const state=await window.iface.agentControl({kind:'read'});const agent=state.agents.find(a=>a.name==='Signed UI fixture');assert(agent,'Agent persisted');
  await tab('Computer');await click('Edit task contract');
  const toggles=[...document.querySelectorAll('.agent-work-controls input[type="checkbox"]')];assert(toggles.length===2&&toggles.every(e=>!e.checked),'Unsafe scripts and consultation default off');
  await fill(field('Expected output'),'Signed UI verified report');await click('Add artifact check');await fill(field('Path'),'report.json');await fill(field('Format'),'json');await click('Save contract');
  await wait(()=>button('Edit task contract'),'contract saved');
  let work=(await window.iface.agentControl({kind:'work',agentId:agent.id,command:{action:'read'}})).work;assert(work.spec.expectedOutput==='Signed UI verified report'&&work.spec.artifacts[0].format==='json','Contract stored by Rust');results.push('task contract saved through UI; risky options default off');
  await tab('Routines');await click('New routine');const form=document.querySelector('.agent-routine-form');await fill(field('Name',form),'Signed calendar');await fill(field('Task',form),'Synthetic scheduled report');await click('Save routine',form);
  await wait(()=>!document.querySelector('.agent-routine-form'),'routine saved');
  const routine=(await window.iface.agentControl({kind:'read'})).routines.find(r=>r.name==='Signed calendar');assert(routine&&!routine.enabled,'First routine paused');
  const files=(await window.iface.agentControl({kind:'readAgentFiles',agentId:agent.id})).files;assert(files.home,'First routine prepared home');results.push('first routine creates home and stays paused');
  const calendar=await wait(()=>document.querySelector('.agent-calendar-controls'),'calendar');calendar.querySelector('summary').click();await wait(()=>calendar.textContent.includes('Current task still needs'),'calendar loaded');await fill(field('IANA timezone',calendar),'America/Chicago');await fill(field('Local time',calendar),'09:15');await click('Preview next runs',calendar);await wait(()=>calendar.textContent.includes('Preview only'),'preview');
  let saved=(await window.iface.agentControl({kind:'calendar',agentId:agent.id,command:{action:'read',routineId:routine.id}})).calendar;assert(saved.schedule===null,'Preview did not save');assert(calendar.querySelectorAll('li').length>0,'Preview occurrences rendered');await click('Save calendar and pause',calendar);
  await wait(async()=>{saved=(await window.iface.agentControl({kind:'calendar',agentId:agent.id,command:{action:'read',routineId:routine.id}})).calendar;return saved.schedule?.hour===9&&saved.schedule?.minute===15},'schedule saved');
  const toggle=await wait(()=>document.querySelector('input[role="switch"][aria-label="Enable Signed calendar"]:not(:disabled)'),'enable switch');toggle.click();await wait(()=>[...document.querySelectorAll('[role="alert"]')].some(e=>/test|verif/i.test(e.textContent)),'enable refused');assert(!toggle.checked,'Untested schedule stays off');results.push('calendar preview, save and test-before-enable refusal');
  await tab('Memory');await wait(()=>document.querySelector('.agent-work-controls'),'memory controls');await fill(field('Fact or preference'),'Synthetic verified preference');await fill(field('Source'),'Signed UI test');await click('Add for review');await click('Confirm fact');
  await wait(async()=>{work=(await window.iface.agentControl({kind:'work',agentId:agent.id,command:{action:'read'}})).work;return work.facts.some(f=>f.text==='Synthetic verified preference'&&f.state==='ownerConfirmed')},'fact confirmation');results.push('memory proposal confirmed through UI and persisted by Rust');
  await tab('Runs');await wait(()=>document.querySelector('.agent-work-controls')?.textContent.includes('Task results'),'results controls');results.push('task result controls rendered');
  return {passed:true,results};
 }.toString()})()`});
 if(response.exceptionDetails)throw new Error(response.exceptionDetails.exception?.description??JSON.stringify(response.exceptionDetails));
 await writeFile(path.join(root,'results.json'),JSON.stringify(response.result.value,null,2));
 const shot=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(root,'agent.png'),Buffer.from(shot.data,'base64'));
 console.log(JSON.stringify({root,...response.result.value}));
} finally {
 ws?.close();for(const p of pending.values())clearTimeout(p.timer);
 child.kill('SIGTERM');for(let i=0;i<40&&child.exitCode===null;i++)await pause(50);if(child.exitCode===null)child.kill('SIGKILL');
 await writeFile(path.join(root,'app.log'),logs);console.log('Private evidence: '+root);
}
