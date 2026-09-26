#!/usr/bin/env python3
"""Black-box OAuth/PKCE through the real MCP binary and an isolated browser shim.
Loopback HTTPS only; no real browser, SSO credentials or Tableau account involved.
"""
import base64, hashlib, json, os, pathlib, socket, ssl, subprocess, sys
import tempfile, threading, urllib.parse
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from contract_tests import MCP

ROOT = pathlib.Path(__file__).resolve().parents[1]
BINARY = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv)>1 else ROOT/'target/release/tabkit'
CLIENT = 'native-fixture-client'
TOKEN = 'syntheticHeader.syntheticOAuthAccessToken.syntheticSignature'
SESSION = 'syntheticOAuthTableauSession'
SITE = '00000000-0000-0000-0000-000000000001'
USER = '00000000-0000-0000-0000-000000000002'
checks = []
def check(name, condition):
    if not condition: raise AssertionError(name)
    checks.append(name)

class State:
    mode = 'normal'
    issuer = ''
    authorization = []
    exchanges = []
    signins = []
    errors = []
s = State()
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *args): pass
    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(data))); self.send_header('Connection','close')
        self.end_headers(); self.wfile.write(data); self.close_connection = True
    def do_GET(self):
        url = urllib.parse.urlsplit(self.path)
        if url.path == '/tenant/.well-known/openid-configuration':
            return self.reply(200, {'issuer':s.issuer if s.mode!='bad_issuer' else s.issuer+'/wrong',
                'authorization_endpoint':s.issuer+'/authorize', 'token_endpoint':s.issuer+'/token'})
        if url.path != '/tenant/authorize': return self.reply(404,{})
        query = urllib.parse.parse_qs(url.query, keep_blank_values=True)
        s.authorization.append(query)
        def one(key): return query.get(key,[None])[0]
        ok = (one('client_id')==CLIENT and one('response_type')=='code'
              and one('code_challenge_method')=='S256' and bool(one('state')))
        if not ok: s.errors.append('invalid authorization request'); return self.reply(400,{})
        code = 'synthetic-code-'+str(len(s.authorization))
        query['_code'] = [code]
        result = {'state':one('state'), 'error':'access_denied'} if s.mode=='denied' else {'state':one('state'),'code':code}
        redirect = one('redirect_uri')+'?'+urllib.parse.urlencode(result)
        self.send_response(302); self.send_header('Location',redirect)
        self.send_header('Content-Length','0'); self.send_header('Connection','close')
        self.end_headers(); self.close_connection = True
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length','0')))
        path = urllib.parse.urlsplit(self.path).path
        if path == '/tenant/token':
            form = urllib.parse.parse_qs(body.decode(),keep_blank_values=True)
            s.exchanges.append(form)
            def one(k): return form.get(k,[None])[0]
            auth = s.authorization[-1]
            challenge = base64.urlsafe_b64encode(hashlib.sha256((one('code_verifier') or '').encode()).digest()).rstrip(b'=').decode()
            ok = (challenge==auth['code_challenge'][0] and one('code')==auth['_code'][0]
                  and one('redirect_uri')==auth['redirect_uri'][0] and one('client_id')==CLIENT
                  and one('grant_type')=='authorization_code' and 'client_secret' not in form)
            if not ok: s.errors.append('invalid token exchange'); return self.reply(400,{})
            if s.mode=='token_error': return self.reply(400,{'error':'invalid_grant','description':'provider-private-detail'})
            return self.reply(200,{'access_token':'opaque-token' if s.mode=='opaque' else TOKEN,'token_type':'Bearer'})
        if path.endswith('/auth/signin'):
            credentials = json.loads(body)['credentials']; s.signins.append(credentials)
            if credentials.get('jwt')!=TOKEN or credentials.get('site',{}).get('contentUrl')!='fixture':
                s.errors.append('wrong JWT sign-in payload'); return self.reply(401,{})
            if s.mode=='tableau_denied': return self.reply(401,{'error':{'code':'401001','detail':'provider-private-detail'}})
            return self.reply(200,{'credentials':{'token':SESSION,'site':{'id':SITE},'user':{'id':USER}}})
        if path.endswith('/auth/signout'):
            if self.headers.get('X-Tableau-Auth')!=SESSION: s.errors.append('wrong sign-out header')
            return self.reply(200,{})
        return self.reply(404,{})

BROWSER = r'''import json, os, pathlib, ssl, sys, urllib.parse, urllib.request
# Deliberately noisy launcher: output must not enter MCP, input must be EOF.
print('BROWSER-NOISE-MUST-NOT-REACH-MCP',flush=True)
print('BROWSER-STDERR-MUST-NOT-REACH-MCP',file=sys.stderr,flush=True)
marker=pathlib.Path(os.environ['TABKIT_BROWSER_MARKER'])
marker.write_text(json.dumps({'stdin_eof':sys.stdin.buffer.read(1)==b''}))
url=sys.argv[1]; parsed=urllib.parse.urlsplit(url)
assert parsed.scheme=='https' and parsed.hostname=='127.0.0.1'
q=urllib.parse.parse_qs(parsed.query); redirect=q['redirect_uri'][0]
# An unrelated request must not consume the pending OAuth flow.
bad=redirect+'?state=unrelated&error=access_denied'
try: urllib.request.urlopen(bad,timeout=2).read()
except urllib.error.HTTPError as e: assert e.code==400
context=ssl.create_default_context(cafile=os.environ['TABKIT_TEST_CA'])
try: urllib.request.urlopen(url,context=context,timeout=5).read()
except urllib.error.HTTPError as e: assert e.code==400
'''

def certificates(work):
    conf=work/'openssl.cnf'; ca=work/'ca.pem'; ca_key=work/'ca-key.pem'
    key=work/'key.pem'; cert=work/'cert.pem'; csr=work/'server.csr'
    conf.write_text('[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=Tabkit OAuth Test Root\n'
        '[root]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n'
        '[leaf]\nsubjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n'
        'extendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature,keyEncipherment\n')
    commands=[
        ['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-config',str(conf),'-extensions','root','-keyout',str(ca_key),'-out',str(ca)],
        ['openssl','req','-new','-newkey','rsa:2048','-nodes','-subj','/CN=127.0.0.1','-keyout',str(key),'-out',str(csr)],
        ['openssl','x509','-req','-in',str(csr),'-CA',str(ca),'-CAkey',str(ca_key),'-CAcreateserial','-days','1','-extfile',str(conf),'-extensions','leaf','-out',str(cert)],
    ]
    for cmd in commands: subprocess.run(cmd,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=30)
    return ca,cert,key

def run():
    if sys.platform!='darwin': raise SystemExit('This browser-shim contract run is currently qualified on macOS only')
    with tempfile.TemporaryDirectory(prefix='tabkit-oauth-contracts-') as td:
        work=pathlib.Path(td); ca,cert,key=certificates(work)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); tls.load_cert_chain(str(cert),str(key))
        server.socket=tls.wrap_socket(server.socket,server_side=True)
        host='https://127.0.0.1:'+str(server.server_port); s.issuer=host+'/tenant'
        thread=threading.Thread(target=server.serve_forever,daemon=True); thread.start()
        launchers=work/'launchers'; launchers.mkdir(); shim=launchers/'open'
        shim.write_text('#!'+sys.executable+'\n'+BROWSER); shim.chmod(0o700)
        marker=work/'browser.json'; env=os.environ.copy()
        env.update({'PATH':str(launchers)+os.pathsep+env.get('PATH',''),
                    'TABKIT_TEST_CA':str(ca),'TABKIT_BROWSER_MARKER':str(marker)})
        def session(mode):
            s.mode=mode
            with socket.socket() as sock:
                sock.bind(('127.0.0.1',0)); port=sock.getsockname()[1]
            args=['--workspace',str(work),'--tableau-server',host,'--tableau-site','fixture',
                '--tableau-auth','oauth','--oauth-issuer',s.issuer,'--oauth-client-id',CLIENT,
                '--oauth-redirect-uri','http://127.0.0.1:'+str(port)+'/callback',
                '--ca-certificate',str(ca),'--request-seconds','4']
            return MCP(args,env)
        def close(m):
            m.err.flush(); m.err.seek(0); exposed=json.dumps(m.seen)+m.err.read().decode(errors='replace')
            secrets=[TOKEN,SESSION,'provider-private-detail','BROWSER-NOISE','BROWSER-STDERR']
            secrets += [x['_code'][0] for x in s.authorization]
            secrets += [x['code_verifier'][0] for x in s.exchanges]
            check('OAuth secrets and launcher output absent from protocol/logs',all(x not in exposed for x in secrets))
            m.close()
        try:
            m=session('normal')
            try:
                r=m.tool('tableau_login',{})
                check('complete HTTPS discovery/authorize/callback/PKCE/Tableau sign-in',r.get('authenticated') is True and len(s.signins)==1 and len(s.exchanges)==1 and not s.errors)
                check('browser launcher has no access to MCP stdin',json.loads(marker.read_text())['stdin_eof'])
                count=len(s.authorization); r=m.tool('tableau_login',{})
                check('active session reused without repeated browser authorization',r.get('authenticated') is True and len(s.authorization)==count)
                m.tool('tableau_logout',{}); r=m.tool('tableau_login',{})
                check('explicit logout permits a fresh OAuth login',r.get('authenticated') is True and len(s.authorization)==count+1)
                check('fresh OAuth attempts use different state and challenge',s.authorization[0]['state']!=s.authorization[1]['state'] and s.authorization[0]['code_challenge']!=s.authorization[1]['code_challenge'])
            finally: close(m)
            expected={'bad_issuer':'OAUTH_ISSUER','denied':'OAUTH_DENIED','token_error':'OAUTH_TOKEN','opaque':'OAUTH_TOKEN','tableau_denied':'AUTH_EXPIRED'}
            for mode,code in expected.items():
                before=(len(s.authorization),len(s.exchanges),len(s.signins)); m=session(mode)
                try:
                    r=m.tool('tableau_login',{})
                    check(mode+' rejected with expected classification',r.get('error',{}).get('code')==code)
                    after=(len(s.authorization),len(s.exchanges),len(s.signins))
                    if mode=='bad_issuer': check('issuer mismatch blocks browser and token endpoints',after==before)
                    elif mode=='denied': check('state-bound denial blocks token exchange',after[1:]==before[1:])
                    elif mode in ('token_error','opaque'): check(mode+' never reaches Tableau sign-in',after[2]==before[2])
                    else: check('Tableau JWT rejection is not automatically replayed',after[2]==before[2]+1)
                finally: close(m)
            check('all request payloads satisfy independently checked contracts',not s.errors)
        finally:
            server.shutdown(); server.server_close(); thread.join(timeout=3)
    return {'passed':len(checks),'failed':0,'checks':checks,'scope':'macOS base executable; loopback TLS OAuth/PKCE with synthetic browser and Tableau; not real corporate SSO'}
if __name__=='__main__': print(json.dumps(run(),indent=2))
