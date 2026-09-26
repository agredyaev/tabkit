#!/usr/bin/env python3
"""Actual tabkit subprocess + loopback HTTPS Tableau fixture. No live credentials.
Uses standard-library TLS/HTTP and subprocesses, not Rust transport mocks.
"""
import base64, hashlib, io, json, os, pathlib, select, socket, ssl
import subprocess, sys, tempfile, threading, time, urllib.parse, zipfile
from email import policy
from email.parser import BytesParser
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

ROOT = pathlib.Path(__file__).resolve().parents[1]
BINARY = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv)>1 else ROOT/'target/debug/tabkit'
SITE='00000000-0000-0000-0000-000000000001'
USER='00000000-0000-0000-0000-000000000002'
PROJECT='00000000-0000-0000-0000-000000000003'
WORKBOOK='00000000-0000-0000-0000-000000000004'
VIEW='00000000-0000-0000-0000-000000000005'
JOB='00000000-0000-0000-0000-000000000006'
PAT='synthetic-PAT-not-a-real-secret-keep-out-of-output'
GOOD=(ROOT/'examples/synthetic.twb').read_bytes()
checks=[]
def check(name, condition):
    if not condition: raise AssertionError(name)
    checks.append(name)
def sha(b):return hashlib.sha256(b).hexdigest()

class Fixture:
    def __init__(self):
        self.requests=[];self.logins=0;self.session='';self.status_next=None
        self.drop_commit=False;self.commits=0;self.uploads=[];self.upload_sessions=0;self.fail_chunk=None
        self.updated='2025-01-01T00:00:00Z';self.override_projects=None
        data=io.BytesIO()
        with zipfile.ZipFile(data,'w',zipfile.ZIP_DEFLATED) as z:
            z.writestr('book.twb',GOOD);z.writestr('Data/Extract.hyper',b'opaque fixture, no native execution')
        self.package=data.getvalue()
    def book(self):return {'id':WORKBOOK,'name':'Contract Book','updatedAt':self.updated,'project':{'id':PROJECT,'name':'Sandbox'},'owner':{'id':USER}}
f=Fixture()
class Handler(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def reply(self,status,value,ctype='application/json'):
        data=value if isinstance(value,bytes) else json.dumps(value).encode()
        self.send_response(status);self.send_header('Content-Type',ctype);self.send_header('Content-Length',str(len(data)))
        self.send_header('Connection','close');self.end_headers();self.wfile.write(data);self.close_connection=True
    def handle_request(self):
        p=urllib.parse.urlsplit(self.path);route=p.path;query=urllib.parse.parse_qs(p.query)
        data=self.rfile.read(int(self.headers.get('Content-Length','0')))
        f.requests.append((self.command,route,query,self.headers.get('X-Tableau-Auth'),data))
        if route.endswith('/auth/signin'):
            c=json.loads(data)['credentials']
            if c.get('personalAccessTokenName')!='test-pat' or c.get('personalAccessTokenSecret')!=PAT:
                return self.reply(401,{'error':{'code':'401001'}})
            f.logins+=1;f.session='synthetic-session-'+str(f.logins)
            return self.reply(200,{'credentials':{'token':f.session,'site':{'id':SITE},'user':{'id':USER}}})
        if self.headers.get('X-Tableau-Auth')!=f.session:return self.reply(401,{'error':{'code':'401002'}})
        if f.status_next is not None:
            status=f.status_next;f.status_next=None
            return self.reply(status,b'not JSON: upstream-secret-must-not-leak','text/plain')
        if route.endswith('/auth/signout'):return self.reply(204,b'')
        if self.command=='GET' and route.endswith('/projects'):
            if f.override_projects is not None:return self.reply(200,f.override_projects)
            return self.reply(200,{'projects':{'project':[{'id':PROJECT,'name':'Sandbox'}]},'pagination':{'totalAvailable':'1'}})
        if self.command=='GET' and route.endswith('/workbooks'):
            return self.reply(200,{'workbooks':{'workbook':[f.book()]},'pagination':{'totalAvailable':'1'}})
        if route.endswith('/workbooks/'+WORKBOOK+'/content'):
            return self.reply(200,f.package,'application/octet-stream')
        if route.endswith('/workbooks/'+WORKBOOK):return self.reply(200,{'workbook':f.book()})
        if route.endswith('/views/'+VIEW+'/image'):
            return self.reply(200,base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=='),'image/png')
        if route.endswith('/views/'+VIEW+'/data'):return self.reply(200,b'Region,Sales\nWest,42\n','text/csv')
        if self.command=='POST' and route.endswith('/fileUploads'):
            f.upload_sessions+=1;f.uploads=[]
            return self.reply(201,{'fileUpload':{'uploadSessionId':'upload-'+str(f.upload_sessions)}})
        if self.command=='PUT' and '/fileUploads/' in route:
            message=BytesParser(policy=policy.default).parsebytes(('Content-Type: '+self.headers['Content-Type']+'\r\nMIME-Version: 1.0\r\n\r\n').encode()+data)
            for part in message.iter_parts():
                if part.get_param('name',header='content-disposition')=='tableau_file':f.uploads.append(part.get_payload(decode=True))
            if f.fail_chunk==len(f.uploads):
                f.fail_chunk=None;return self.reply(500,{'error':{'code':'500000'}})
            return self.reply(200,{'fileUpload':{'uploadSessionId':'upload-'+str(f.upload_sessions)}})
        if self.command=='POST' and route.endswith('/workbooks'):
            f.commits+=1
            if f.drop_commit:
                f.drop_commit=False;self.close_connection=True
                self.connection.shutdown(socket.SHUT_RDWR);self.connection.close();return
            return self.reply(202,{'job':{'id':JOB,'mode':'Asynchronous'}})
        if route.endswith('/jobs/'+JOB):return self.reply(200,{'job':{'id':JOB,'progress':'100','finishCode':'0','completedAt':'2025-01-01T00:00:01Z'}})
        return self.reply(404,{'error':{'code':'404000'}})
    do_GET=do_POST=do_PUT=handle_request

class MCP:
    def __init__(self,args,env):
        self.err=tempfile.TemporaryFile();self.p=subprocess.Popen([str(BINARY)]+args+['mcp'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.err,env=env)
        self.seq=0;self.buffer=b'';self.seen=[]
        self.rpc('initialize',{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'contract-tests','version':'1'}})
        self.p.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n');self.p.stdin.flush()
    def rpc(self,method,params):
        self.seq+=1;ident=self.seq
        self.p.stdin.write(json.dumps({'jsonrpc':'2.0','id':ident,'method':method,'params':params}).encode()+b'\n');self.p.stdin.flush()
        deadline=time.monotonic()+12
        while time.monotonic()<deadline:
            if b'\n' in self.buffer:
                line,self.buffer=self.buffer.split(b'\n',1)
                message=json.loads(line);self.seen.append(message)
                if message.get('id')==ident:
                    if 'error' in message:raise AssertionError('protocol error: '+str(message['error']))
                    return message['result']
                continue
            ready,_,_=select.select([self.p.stdout],[],[],max(0,deadline-time.monotonic()))
            if not ready:break
            block=os.read(self.p.stdout.fileno(),65536)
            if not block:raise AssertionError('MCP exited before result')
            self.buffer+=block
        raise AssertionError('MCP response timeout')
    def tool(self,name,args):
        result=self.rpc('tools/call',{'name':name,'arguments':args})
        texts=[c['text'] for c in result['content'] if c['type']=='text']
        return json.loads(texts[0])
    def close(self):
        if self.p.stdin and not self.p.stdin.closed:self.p.stdin.close()
        try:self.p.wait(timeout=3)
        except subprocess.TimeoutExpired:self.p.kill();self.p.wait()
        self.err.seek(0);stderr=self.err.read().decode(errors='replace');self.err.close()
        check('credentials absent from MCP/stdout/stderr',PAT not in json.dumps(self.seen)+stderr and 'synthetic-session-' not in json.dumps(self.seen)+stderr and 'upstream-secret' not in json.dumps(self.seen)+stderr)

def run():
    with tempfile.TemporaryDirectory(prefix='tabkit-contracts-') as temp:
        work=pathlib.Path(temp);cert=work/'cert.pem';key=work/'key.pem';conf=work/'openssl.cnf'
        ca=work/'ca.pem';ca_key=work/'ca-key.pem';csr=work/'server.csr'
        conf.write_text('[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=Tabkit Test Root\n[root]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n[leaf]\nsubjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature,keyEncipherment\n')
        commands=[
            ['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-config',str(conf),'-extensions','root','-keyout',str(ca_key),'-out',str(ca)],
            ['openssl','req','-new','-newkey','rsa:2048','-nodes','-subj','/CN=127.0.0.1','-keyout',str(key),'-out',str(csr)],
            ['openssl','x509','-req','-in',str(csr),'-CA',str(ca),'-CAkey',str(ca_key),'-CAcreateserial','-days','1','-extfile',str(conf),'-extensions','leaf','-out',str(cert)],
        ]
        for command in commands:subprocess.run(command,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=30)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(str(cert),str(key));server.socket=tls.wrap_socket(server.socket,server_side=True)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        env=os.environ.copy();env['TABKIT_TEST_PAT']=PAT
        args=['--workspace',str(work),'--tableau-server','https://127.0.0.1:'+str(server.server_port),'--tableau-site','test-site','--tableau-auth','pat','--tableau-pat-name','test-pat','--tableau-pat-secret-env','TABKIT_TEST_PAT','--request-seconds','3','--allow-data-output','--allow-unverified-formula-edits','--publish-enabled','--publish-project',PROJECT,'--allow-overwrite']
        # The executable must reject the local self-signed server without explicit trust.
        untrusted=subprocess.run([str(BINARY)]+args+['call','tableau_login'],input=b'{}',stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env,timeout=8)
        check('TLS rejects untrusted certificate before PAT reaches HTTP',untrusted.returncode!=0 and f.logins==0)
        args+=['--ca-certificate',str(ca)]
        m=None
        try:
            m=MCP(args,env)
            r=m.tool('tableau_explore',{'resource':'projects'})
            check('PAT sign-in and authenticated HTTPS explore: '+str(r),r.get('items',[{}])[0].get('id')==PROJECT and f.logins==1)
            calls=len(f.requests);f.status_next=401
            r=m.tool('tableau_explore',{'resource':'projects'})
            check('401 returns error without automatic replay',r.get('error') is not None and len(f.requests)==calls+1 and f.logins==1)
            r=m.tool('tableau_explore',{'resource':'projects'})
            check('next request reauthenticates after rejected session',r['items'][0]['id']==PROJECT and f.logins==2)
            f.status_next=403;r=m.tool('tableau_explore',{'resource':'projects'});m.tool('tableau_explore',{'resource':'projects'})
            check('403 does not switch or recreate identity',r.get('error') is not None and f.logins==2)
            r=m.tool('tableau_search',{'resource':'workbooks','text':'contract','project_id':PROJECT})
            check('search uses real REST pages and reports completeness',len(r['items'])==1 and r['truncated'] is False)
            r=m.tool('tableau_download_workbook',{'workbook_id':WORKBOOK,'output':'download.twbx'})
            check('download verifies and commits actual TWBX',r['sha256']==sha(f.package) and (work/'download.twbx').read_bytes()==f.package)
            check('download explicitly requests extracts',any(x[2].get('includeExtract')==['true'] for x in f.requests))
            count=len(f.requests);r=m.tool('tableau_download_workbook',{'workbook_id':WORKBOOK,'output':'download.twbx'})
            check('existing download rejected before network',r['error']['code']=='OUTPUT_EXISTS' and len(f.requests)==count)
            r=m.tool('tableau_view_image',{'view_id':VIEW,'output':'view.png','filters':{}})
            check('image path returns valid PNG artifact',r['format']=='png' and (work/'view.png').read_bytes().startswith(b'\x89PNG'))
            r=m.tool('tableau_view_data',{'view_id':VIEW,'output':'view.csv','filters':{'Region':'West'}})
            check('CSV export and filter encoding',r['format']=='csv' and any(x[2].get('vf_Region')==['West'] for x in f.requests))
            p=m.tool('workbook_plan',{'input':'download.twbx','output':'change.plan.json','changes':{'schema_version':1,'input_sha256':sha(f.package),'operations':[{'op':'set_calculation','field_id':3,'expected_formula':'[Profit] / [Sales]','formula':'[Profit] / ([Sales] + 1)'}]}})
            out=m.tool('workbook_apply',{'plan':'change.plan.json','expected_plan_sha256':p['plan_sha256'],'output':'candidate.twbx'})
            candidate=(work/'candidate.twbx').read_bytes()
            with zipfile.ZipFile(io.BytesIO(candidate)) as z:check('download/edit preserves opaque extract',z.read('Data/Extract.hyper')==b'opaque fixture, no native execution')
            def prepare(input='candidate.twbx',hash_value=None,overwrite=None):
                return m.tool('tableau_prepare_publish',{'input':input,'expected_sha256':hash_value or out['sha256'],'name':'Contract Book','project_id':PROJECT,'overwrite_workbook_id':overwrite,'acknowledge_tableau_not_run':True,'acknowledge_unsupported_objects':True})
            def publish(a):return m.tool('tableau_publish',{'approval_id':a['approval_id'],'expected_approval_sha256':a['approval_sha256'],'confirm':True})
            bad=(ROOT/'tests/fixtures/invalid-parameter.twb').read_bytes();(work/'bad.twb').write_bytes(bad);count=len(f.requests)
            r=prepare('bad.twb',sha(bad));check('invalid parameter blocks publish before network',r['error']['code']=='LOCAL_VALIDATION_FAILED' and len(f.requests)==count)
            count=f.commits;a=prepare();check('prepare performs no upload or commit',f.commits==count and f.upload_sessions==0)
            r=publish(a);check('publish returns async job receipt',r['outcome']['status']=='submitted' and r['outcome']['job_id']==JOB)
            check('uploaded multipart payload is exact candidate',b''.join(f.uploads)==candidate)
            check('new-copy publish explicitly disables overwrite',any(x[0]=='POST' and x[1].endswith('/workbooks') and x[2].get('overwrite')==['false'] for x in f.requests))
            count=f.commits;r=publish(a);check('same approval cannot publish twice',r['error']['code']=='ALREADY_ATTEMPTED' and f.commits==count)
            r=m.tool('tableau_publish_receipt',{'approval_id':a['approval_id']});check('receipt persists exact outcome',r['outcome']['job_id']==JOB)
            r=m.tool('tableau_get_job',{'id':JOB});check('job read reports actual completion fields',r['finishCode']=='0' and r['progress']=='100')
            a=prepare(overwrite=WORKBOOK);f.updated='2025-01-02T00:00:00Z';count=f.commits;r=publish(a)
            check('changed overwrite baseline blocks commit',r['error']['code']=='REMOTE_CHANGED' and f.commits==count)
            a=prepare();f.drop_commit=True;count=f.commits;r=publish(a)
            check('dropped commit response is unknown not success',r['status']=='failed_or_unknown' and r['error']['code']=='HTTP_OUTCOME_UNKNOWN' and f.commits==count+1)
            r=publish(a);check('unknown outcome is not replayed',r['error']['code']=='ALREADY_ATTEMPTED' and f.commits==count+1)
            # Exercise the actual 8 MiB multipart chunk boundary, not a one-chunk stand-in.
            large=GOOD.replace(b'</workbook>',b'<!--'+b'x'*(8*1024*1024)+b'--></workbook>')
            (work/'large.twb').write_bytes(large)
            a=prepare('large.twb',sha(large));r=publish(a)
            check('multi-chunk upload preserves order and all bytes',r['outcome']['status']=='submitted' and len(f.uploads)==2 and b''.join(f.uploads)==large)
            a=prepare('large.twb',sha(large));f.fail_chunk=2;count=f.commits;r=publish(a)
            check('second-chunk failure stops before final commit',r['status']=='failed_or_unknown' and len(f.uploads)==2 and f.commits==count)
            r=publish(a);check('partial upload failure is not replayed',r['error']['code']=='ALREADY_ATTEMPTED' and f.commits==count)
            r=m.tool('tableau_logout',{});check('logout clears identity and signs out',r['authenticated'] is False)
        finally:
            if m:m.close()
            server.shutdown();server.server_close();thread.join(timeout=3)
    return {'passed':len(checks),'failed':0,'checks':checks,'scope':'actual CLI/MCP executable against loopback TLS Tableau fixture; no live service'}

if __name__=='__main__':
    result=run();print(json.dumps(result,indent=2))
