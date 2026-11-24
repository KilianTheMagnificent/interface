"""Synthetic macOS release-host acceptance suite. No real account or provider is used.
Run with the exact interface-host extracted from a verified production ZIP.
An optional second argument pins its expected wire version (default: 29).
Every host/provider process is sandboxed; all fixtures and results are disposable.
"""
import json,os,pathlib,queue,subprocess,threading,time,uuid,shutil,sys,datetime
root=pathlib.Path('/private/tmp')/('ip-'+uuid.uuid4().hex[:8]);root.mkdir(mode=0o700)
for name in ('home','data','process-home','tmp','bin'):(root/name).mkdir(mode=0o700)
source=pathlib.Path(sys.argv[1]);host=root/'interface-host';shutil.copy2(source,host)
for engine in ('claude','codex'):
 target=root/'bin'/engine;shutil.copy2(pathlib.Path(__file__).with_name('agent-production-fake-provider.py'),target);target.chmod(0o700)
for engine in ('grok','qwen','agy','omp','opencode','qwen-mlx-server'):
 target=root/'bin'/engine;target.write_text('#!/bin/sh\nexit 64\n');target.chmod(0o700)
for home in ('home','process-home'):(root/home/'.zprofile').write_text('export PATH='+str(root/'bin')+':/usr/bin:/bin:/usr/sbin:/sbin\n')
policy='''(version 1)(allow default)(deny file-write*)(allow file-write* (subpath %s) (subpath "/dev"))(deny file-read* (subpath %s))(deny network*)(allow network-bind (local ip "localhost:*"))(allow network-inbound (local ip "localhost:*"))(allow network-outbound (remote ip "localhost:*"))(allow network* (local unix-socket) (remote unix-socket))'''%(json.dumps(str(root)),json.dumps(str(pathlib.Path.home())))
env=dict(HOME=str(root/'process-home'),CFFIXED_USER_HOME=str(root/'process-home'),PATH=str(root/'bin')+':/usr/bin:/bin:/usr/sbin:/sbin',SHELL='/bin/zsh',LANG='en_US.UTF-8',TMPDIR=str(root/'tmp'),ZDOTDIR=str(root/'process-home'),PYTHONDONTWRITEBYTECODE='1',INTERFACE_HOST_SMOKE_DATA_ROOT=str(root/'data'),INTERFACE_HOST_SMOKE_HOME=str(root/'home'),INTERFACE_VOICE_HELPER='/usr/bin/false',CLAUDE_CODE_EXECUTABLE=str(root/'bin/claude'),CODEX_EXECUTABLE=str(root/'bin/codex'))
protocol=int(sys.argv[2]) if len(sys.argv)>2 else 29
q=None;p=None;counter=0;results=[]
def launch():
 global p,q
 q=queue.Queue();log=open(root/'host.log','a')
 p=subprocess.Popen(['/usr/bin/sandbox-exec','-p',policy,str(host)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,env=env,cwd=root)
 current, incoming = p, q
 threading.Thread(target=lambda:[incoming.put(json.loads(line)) for line in current.stdout],daemon=True).start()
 ready=q.get(timeout=30);assert ready['kind']=='ready' and ready['version']==protocol,ready

def request(operation,payload,timeout=40):
 global counter
 counter+=1;id=str(counter);p.stdin.write(json.dumps(dict(version=protocol,id=id,operation=operation,payload=payload))+'\n');p.stdin.flush();end=time.monotonic()+timeout
 while time.monotonic()<end:
  frame=q.get(timeout=max(.1,end-time.monotonic()))
  if frame.get('kind')=='response' and frame.get('id')==id:
   if not frame.get('ok'):raise RuntimeError(json.dumps(frame.get('error')))
   return frame.get('value',frame.get('result'))
 raise TimeoutError(operation)
def cmd(**payload):return request('iface:agentControl',payload)
def work(agent,action='read',**kw):return cmd(kind='work',agentId=agent,command=dict(action=action,**kw))
def check(name,fn):
 try:detail=fn();r=dict(name=name,passed=True,detail=detail)
 except Exception as error:r=dict(name=name,passed=False,error=str(error))
 results.append(r);print(json.dumps(r),flush=True)
def stop():
 if p and p.poll() is None:
  p.stdin.close()
  try:p.wait(timeout=15)
  except subprocess.TimeoutExpired:p.kill();p.wait()
def create(agent,engine):
 state=cmd(kind='read');return cmd(kind='saveAgent',expectedRevision=state['revision'],profile=dict(id=agent,name='Synthetic '+agent,engine=engine,model='synthetic',effort='low',instructions='Synthetic sandbox fixture only.',color='sky'))
def configure(agent,**extra):
 state=work(agent)['work'];spec=state['spec'];spec.update(expectedOutput='Synthetic JSON artifact',artifacts=[dict(path='report.json',format='json',contains='production-sandbox-ok')]);spec.update(extra)
 return work(agent,'configure',expectedRevision=state['revision'],spec=spec)
def start(agent,text):return cmd(kind='startAgentThread',agentId=agent,requestId=uuid.uuid4().hex,text=text)
def settle(agent):
 for _ in range(120):
  state=work(agent);task=state['work']['tasks'][-1]
  if task['execution'] in ('completed','failed','unknown','stopped'):return task
  time.sleep(.1)
 raise TimeoutError('task did not settle')
def success(engine):
 agent='ok-'+engine;create(agent,engine);configure(agent);start(agent,'TOOLS success');task=settle(agent);assert task['execution']=='completed',task
 task=work(agent,'verify',taskId=task['id'])['work']['tasks'][-1];assert task['verification']=='verified',task
 assert 'Agent request: production sandbox host tools' in task['notes'],task
 return dict(execution=task['execution'],verification=task['verification'])
def artifact_failure(mode):
 agent=mode.lower();create(agent,'codex');configure(agent);start(agent,mode);task=settle(agent)
 checked=work(agent,'verify',taskId=task['id'])['work']['tasks'][-1];assert checked['verification']!='verified',checked
 return dict(execution=checked['execution'],verification=checked['verification'])
def stop_running():
 create('stopping','codex');configure('stopping');start('stopping','HANG');task=work('stopping')['work']['tasks'][-1]
 checked=work('stopping','stop',taskId=task['id'])['work']['tasks'][-1];assert checked['execution']=='stopped',checked;return checked['execution']
def memory():
 agent='memory';create(agent,'codex');cmd(kind='recreateAgentHome',agentId=agent);files=cmd(kind='readAgentFiles',agentId=agent)['files'];original=files['memory'];cmd(kind='saveAgentMemory',agentId=agent,expectedSha256=original['sha256'],text='Synthetic remembered note\n')
 state=work(agent)['work'];assert state['memoryHistory'],state
 files=cmd(kind='readAgentFiles',agentId=agent)['files'];work(agent,'restoreMemory',sha256=original['sha256'],expectedSha256=files['memory']['sha256'])
 restored=cmd(kind='readAgentFiles',agentId=agent)['files']['memory'];assert restored['text']==original['text']
 fact=dict(id='f1',text='Synthetic fact',source='sandbox fixture',observedAtMs=int(time.time()*1000),expiresAtMs=None,supersedes=None,state='proposed');work(agent,'proposeFact',fact=fact)
 state=work(agent,'confirmFact',id='f1')['work'];assert state['facts'][0]['state']=='ownerConfirmed';return 'memory restored; fact owner-confirmed'
def calendar():
 create('calendar','codex');schedule=dict(timezone='America/Chicago',weekdays=[],hour=2,minute=30,missed='skip',pauseOnFailure=True)
 after=int(datetime.datetime(2027,3,13,12,tzinfo=datetime.timezone.utc).timestamp()*1000)
 view=cmd(kind='calendar',agentId='calendar',command=dict(action='preview',schedule=schedule,afterMs=after))['calendar'];times=view['nextOccurrencesMs'];assert times==sorted(set(times)) and len(times)>1,view
 dates=[datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).isoformat() for t in times];assert not any(d.startswith('2027-03-14') for d in dates),dates
 schedule['hour']=1
 after=int(datetime.datetime(2027,11,6,12,tzinfo=datetime.timezone.utc).timestamp()*1000)
 folded=cmd(kind='calendar',agentId='calendar',command=dict(action='preview',schedule=schedule,afterMs=after))['calendar']['nextOccurrencesMs']
 fall=[datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).isoformat() for t in folded]
 assert fall[0]=='2027-11-07T06:30:00+00:00' and fall[1]=='2027-11-08T07:30:00+00:00',fall
 return dict(springGap=dates,fallFold=fall)
def recovery():
 create('recovery','codex');configure('recovery');start('recovery','HANG')
 task=work('recovery')['work']['tasks'][-1];assert task['execution'] in ('starting','running'),task
 p.kill();p.wait(timeout=10);launch()
 recovered=work('recovery')['work']['tasks'][-1];assert recovered['execution']=='unknown',recovered
 try:request('iface:resumeSession',dict(sessionKey=recovered['sessionKey']))
 except RuntimeError as error:
  assert any(x in str(error).lower() for x in ('acknowledge','uncertain','unknown')),str(error)
 else:raise AssertionError('uncertain work replayed')
 work('recovery','acknowledge',taskId=recovered['id'])
 return 'restart marks Unknown; restart refused until explicit acknowledgement'
def routine_gate():
 agent='routine';create(agent,'codex');configure(agent)
 routine=dict(id='r1',agentId=agent,name='Synthetic daily',prompt='success',cwd='',enabled=True,startAtMs=int(time.time()*1000)+86400000,intervalMinutes=1440,nextRunAtMs=None)
 routine['enabled']=False;state=cmd(kind='read');state=cmd(kind='saveRoutine',expectedRevision=state['revision'],routine=routine)
 schedule=dict(timezone='America/Chicago',weekdays=[],hour=9,minute=0,missed='skip',pauseOnFailure=True)
 state=cmd(kind='calendar',agentId=agent,command=dict(action='save',expectedRevision=state['revision'],routineId='r1',schedule=schedule))
 routine=next(r for r in state['routines'] if r['id']=='r1');routine['enabled']=True
 try:cmd(kind='saveRoutine',expectedRevision=state['revision'],routine=routine)
 except RuntimeError as error:assert 'test' in str(error).lower(),str(error)
 else:raise AssertionError('untested calendar routine was enabled')
 state=cmd(kind='read')
 cmd(kind='runNow',expectedRevision=state['revision'],routineId='r1',requestId='manual-test')
 for _ in range(150):
  tasks=work(agent)['work']['tasks']
  if tasks and tasks[-1]['execution']=='completed':break
  time.sleep(.1)
 else:raise TimeoutError('manual routine')
 work(agent,'verify',taskId=tasks[-1]['id'])
 view=cmd(kind='calendar',agentId=agent,command=dict(action='read',routineId='r1'))['calendar'];assert view['tested'],view
 state=cmd(kind='read');routine=next(r for r in state['routines'] if r['id']=='r1');routine['enabled']=True
 state=cmd(kind='saveRoutine',expectedRevision=state['revision'],routine=routine)
 assert next(r for r in state['routines'] if r['id']=='r1')['enabled']
 configure(agent,expectedOutput='Changed contract requires a new test')
 changed=cmd(kind='calendar',agentId=agent,command=dict(action='read',routineId='r1'))['calendar'];assert not changed['tested'],changed
 return 'untested enable refused; verified manual test permits enable; changed contract invalidates test'
def overlap():
 shared=root/'shared';shared.mkdir();agents=['lease-one','lease-two']
 for agent in agents:
  create(agent,'codex');configure(agent);state=cmd(kind='read');cmd(kind='saveAgentFolders',expectedRevision=state['revision'],agentId=agent,folders=[str(shared)])
 start(agents[0],'HANG')
 try:start(agents[1],'success')
 except RuntimeError as error:assert any(x in str(error).lower() for x in ('overlap','lease','resource','active')),str(error)
 else:raise AssertionError('overlapping write lease admitted')
 task=work(agents[0])['work']['tasks'][-1];work(agents[0],'stop',taskId=task['id'])
 return 'overlapping writable scope refused'

def consultation():
 agent='consult';create(agent,'codex');state=cmd(kind='recreateAgentHome',agentId=agent);home=pathlib.Path(next(a for a in state['agents'] if a['id']==agent)['home']);target=home/'selected.txt';target.write_text('before child\n')
 configure(agent,consultation=dict(model='claude/synthetic',files=['selected.txt'],writableFiles=['selected.txt'],maxChildren=1))
 start(agent,'CONSULT TOOLS');task=settle(agent);assert task['execution']=='completed',task
 state=work(agent,'verify',taskId=task['id'])['work'];assert state['tasks'][-1]['verification']=='verified',state
 child=state['consultations'][-1];assert child['status']=='completed' and child['review']=='accepted',child
 assert target.read_text()=='updated by synthetic child\n';assert child['proposals'][0]['appliedSha256'];return 'selected context, tool-free child, reviewed proposal and host CAS applied'
def approvals():
 agent='approval';create(agent,'claude');start(agent,'DENY')
 for _ in range(100):
  state=work(agent);pending=[a for a in state['pendingApprovals'] if a['agentId']==agent]
  if pending:break
  time.sleep(.1)
 else:raise TimeoutError('outside write did not require approval')
 for a in pending:request('iface:respond',dict(sessionKey=a['sessionKey'],approvalId=a['approvalId'],verdict='deny'))
 task=settle(agent);assert task['execution']=='completed',task
 home=pathlib.Path(task['cwd']);response=json.loads((home/'approval-response.json').read_text());assert response['response']['response']['behavior']=='deny',response
 assert not (root/'not-granted.txt').exists();return 'outside-scope write asked; owner denial reached provider; no write'
def resume_tools():
 agent='resume';create(agent,'codex');configure(agent);start(agent,'TOOLS');task=settle(agent);key=task['sessionKey'];work(agent,'stop',taskId=task['id'])
 request('iface:resumeSession',dict(sessionKey=key));request('iface:send',dict(sessionKey=key,text='TOOLS resumed'));task=settle(agent)
 assert task['execution']=='completed' and 'Agent request: production sandbox host tools' in task['notes'],task
 return 'scoped host tools work after explicit resume'

def deadline():
 agent='deadline';create(agent,'codex');configure(agent,budget=dict(wallSeconds=10,noProgressSeconds=10,maxActions=200,maxReportedTokens=None));began=time.monotonic();start(agent,'HANG')
 for _ in range(220):
  task=work(agent)['work']['tasks'][-1]
  if task['execution']=='stopped':break
  time.sleep(.1)
 else:raise TimeoutError('deadline did not stop task')
 assert 'deadline' in task['detail'].lower() or 'budget' in task['detail'].lower(),task
 return dict(execution=task['execution'],seconds=round(time.monotonic()-began,2))
def stale_input():
 agent='stale';create(agent,'codex');state=cmd(kind='recreateAgentHome',agentId=agent);home=pathlib.Path(next(a for a in state['agents'] if a['id']==agent)['home']);target=home/'input.json';target.write_text('{}');os.utime(target,(time.time()-3600,time.time()-3600));configure(agent,inputs=[dict(path='input.json',maxAgeSeconds=1)])
 try:start(agent,'success')
 except RuntimeError as error:assert any(x in str(error).lower() for x in ('old','fresh','stale')),str(error)
 else:raise AssertionError('stale input reached provider')
 return 'stale required input refused before dispatch'
def revoke():
 agent='revocation';create(agent,'codex');configure(agent);shared=root/'revoked';shared.mkdir();state=cmd(kind='read');cmd(kind='saveAgentFolders',expectedRevision=state['revision'],agentId=agent,folders=[str(shared)]);start(agent,'HANG');task=work(agent)['work']['tasks'][-1];key=task['sessionKey']
 state=cmd(kind='read');state=cmd(kind='saveAgentFolders',expectedRevision=state['revision'],agentId=agent,folders=[]);assert next(a for a in state['agents'] if a['id']==agent).get('folders',[])==[]
 request('iface:resumeSession',dict(sessionKey=key));request('iface:send',dict(sessionKey=key,text='TOOLS after revocation'));task=settle(agent)
 assert task['execution']=='completed' and str(shared) not in task['roots'],task
 return 'live provider closed; resumed task has only the reduced roots'

launch()
try:
 for engine in ('claude','codex'):check(engine+' success, artifact verification and scoped host tools',lambda engine=engine:success(engine))
 for mode in ('BADJSON','MISSING'):check(mode+' artifact cannot verify',lambda mode=mode:artifact_failure(mode))
 check('Stop active task',stop_running);check('Memory history, restore and fact confirmation',memory);check('DST spring gap and fall fold previews',calendar);check('Routine test-before-enable',routine_gate);check('Exclusive folder leases',overlap);check('Scoped consultation review and apply',consultation);check('Approval denial for outside-scope write',approvals);check('Resume scoped host tools',resume_tools);check('Required input freshness',stale_input);check('Active grant revocation',revoke);check('Automatic budget stop',deadline);check('Crash recovery does not replay',recovery)
finally:
 stop();(root/'results.json').write_text(json.dumps(results,indent=2)+'\n');print('Evidence: '+str(root/'results.json'),flush=True)
sys.exit(0 if all(r['passed'] for r in results) else 1)
