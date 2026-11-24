#!/usr/bin/python3
import json,sys,os,pathlib,socket,uuid,time,traceback,hashlib
sys.excepthook=lambda t,e,b: pathlib.Path("fake-error.txt").write_text("".join(traceback.format_exception(t,e,b)))
if '--version' in sys.argv:
 print('1.0.0');sys.exit()
claude='claude' in pathlib.Path(sys.argv[0]).name
sid='sandbox-'+uuid.uuid4().hex[:10];endpoint=None;turn=0;mode=''
for flag in ('--session-id','--resume'):
 if flag in sys.argv:sid=sys.argv[sys.argv.index(flag)+1]
 for arg in sys.argv:
  if arg.startswith(flag+'='):sid=arg.split('=',1)[1]
def emit(value):
 with open("fake-output.jsonl","a") as log: log.write(json.dumps(value)+"\n")
 print(json.dumps(value),flush=True)
def tool(name,args):
 s=socket.socket(socket.AF_UNIX);s.settimeout(10);s.connect(endpoint)
 s.sendall((json.dumps(dict(jsonrpc='2.0',id=1,method='tools/list' if name is None else 'tools/call',params={} if name is None else dict(name=name,arguments=args)))+'\n').encode())
 result=json.loads(s.makefile().readline());s.close()
 if result.get('error') or result.get('result',{}).get('isError'):raise RuntimeError(result)
 return result

if '--json-schema' in sys.argv:
 packet=sys.stdin.read()
 assert 'selected.txt' in packet and 'before child' in packet
 emit(dict(type='system',subtype='init',session_id=sid,model='synthetic',tools=[],mcp_servers=[],plugins=[],skills=[]))
 emit(dict(type='result',subtype='success',is_error=False,session_id=sid,structured_output=dict(version=1,outcome='changes',summary='Synthetic selected-file proposal',missingFiles=[],files=[dict(path='selected.txt',operation='write',content='updated by synthetic child\n')]),usage=dict(input_tokens=10,output_tokens=5)))
 sys.exit(0)
def value(result):
 data=json.loads(result['result']['content'][0]['text'])
 if isinstance(data,dict) and 'error' in data:raise RuntimeError(data)
 return data

def execute(text):
 global mode
 mode=text
 if 'HANG' in text:return
 if 'DENY' in text:
  emit(dict(type='control_request',request_id='outside-write',request=dict(subtype='can_use_tool',tool_name='Write',tool_use_id='outside-write',input=dict(file_path=str(pathlib.Path(os.environ['INTERFACE_HOST_SMOKE_DATA_ROOT']).parent/'not-granted.txt'),content='must not appear'))))
  return
 if 'CONSULT' in text:
  child=value(tool('consult_start',dict(requestId='child-1',brief='Update only selected.txt from the provided synthetic content.')))
  value(tool('consult_review',dict(requestId='review-1',childId=child['id'],resultSha256=child['resultSha256'],verdict='accepted')))
  value(tool('consult_apply',dict(requestId='apply-1',childId=child['id'],resultSha256=child['resultSha256'],path=child['proposals'][0]['path'])))
 if 'CRASH' in text:os._exit(9)
 if 'BADJSON' in text:pathlib.Path('report.json').write_text('{\"marker\":\"production-sandbox-ok\", invalid}')
 elif 'MISSING' not in text:pathlib.Path('report.json').write_text('{"marker":"production-sandbox-ok"}\n')
 if endpoint and 'TOOLS' in text:
  catalogue=tool(None,{})['result']['tools']
  assert sorted(t['name'] for t in catalogue)==sorted(['task_status','artifact_register','memory_propose','notification_request','routine_draft','consult_start','consult_review','consult_apply'])
  memory=pathlib.Path('MEMORY.md');before=hashlib.sha256(memory.read_bytes()).hexdigest()
  value(tool('memory_propose',dict(requestId='memory-'+str(turn),expectedSha256=before,text='Synthetic agent memory proposal\n')))
  draft=dict(requestId='draft-'+str(turn),name='Synthetic draft',prompt='Requires owner review')
  first=value(tool('routine_draft',draft));second=value(tool('routine_draft',draft));assert first==second and first['enabled']==False
  receipts=[tool('task_status',{}),tool('artifact_register',dict(requestId='artifact-'+str(turn),path='report.json')),tool('notification_request',dict(requestId='note-'+str(turn),text='production sandbox host tools'))]
  pathlib.Path('tool-receipts.json').write_text(json.dumps(receipts))
 if claude:
  emit(dict(type='assistant',message=dict(role='assistant',content=[dict(type='text',text='Synthetic task finished')]),session_id=sid))
  emit(dict(type='result',subtype='success',is_error=False,session_id=sid,usage=dict(input_tokens=10,output_tokens=5)))
 else:
  emit(dict(method='turn/completed',params=dict(threadId=sid,turn=dict(id='turn-'+str(turn),status='completed'))))
  emit(dict(method='thread/status/changed',params=dict(threadId=sid,status=dict(type='idle'))))

if claude:
 for i,a in enumerate(sys.argv):
  if a=='--mcp-config':
   try:
    config=json.loads(sys.argv[i+1]);endpoint=config['mcpServers']['iface_agent']['args'][-1]
   except Exception:pass
for line in sys.stdin:
 try:msg=json.loads(line)
 except Exception:continue
 if claude:
  if msg.get('type')=='control_request':emit(dict(type='control_response',response=dict(subtype='success',request_id=msg['request_id'],response={})))
  elif msg.get('type')=='control_response':
   pathlib.Path('approval-response.json').write_text(json.dumps(msg))
   execute('approved response completed')
  elif msg.get('type')=='user':
   turn+=1;emit(dict(type='system',subtype='init',session_id=sid,model='synthetic',cwd=os.getcwd(),tools=[]))
   execute(str(msg.get('message',{}).get('content','')))
  continue
 method=msg.get('method');params=msg.get('params',{});result={}
 if 'id' not in msg:continue
 if method=='config/read':result=dict(config=dict(model=None,mcp_servers={}))
 elif method in ('thread/start','thread/resume'):
  sid=params.get('threadId',sid)
  os.chdir(params.get('cwd',os.getcwd()))
  roots=params.get('config',{}).get('sandbox_workspace_write',{}).get('writable_roots',[])
  result=dict(thread=dict(id=sid),model='synthetic',cwd=params.get('cwd',os.getcwd()),approvalPolicy='untrusted',approvalsReviewer='user',sandbox=dict(type='workspaceWrite',writableRoots=roots,networkAccess=True,excludeSlashTmp=True,excludeTmpdirEnvVar=False))
  tools=params.get('config',{}).get('mcp_servers',{}).get('iface_agent',{});endpoint=tools.get('args',[None])[-1]
 elif method=='turn/start':
  turn+=1;result=dict(turn=dict(id='turn-'+str(turn)))
 emit(dict(id=msg['id'],result=result))
 if method=='turn/start':
  emit(dict(method='turn/started',params=dict(threadId=sid,turn=dict(id='turn-'+str(turn)))))
  execute(' '.join(item.get('text','') for item in params.get('input',[])))
