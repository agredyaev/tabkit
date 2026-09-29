#!/usr/bin/env python3
"""Bounded coverage-guided fuzz run. Never swallows crashes or fabricates a pass."""
import argparse,hashlib,json,os,pathlib,re,shutil,signal,subprocess,time
ROOT=pathlib.Path(__file__).resolve().parents[1]
TARGETS=['wire_json','xml_workbook','lineage','formula','patches','scalars','sql_read','oauth_callback','twbx_package','twbx_structured','workbook_edits','wire_frames','edit_sequences']
p=argparse.ArgumentParser();p.add_argument('--seconds',type=int,default=60);p.add_argument('--toolchain',default='nightly');p.add_argument('--report',default='reports/fuzz');p.add_argument('--target',action='append',choices=TARGETS);p.add_argument('--seed-base',type=int,default=2026092600);p.add_argument('--fresh-corpus',action='store_true')
a=p.parse_args()
if not 1<=a.seconds<=3600:p.error('seconds must be 1..3600')
if os.name!='posix':p.error('This bounded runner currently requires POSIX process groups')
folder=ROOT/a.report;folder.mkdir(parents=True,exist_ok=True)
env=os.environ.copy();env['CARGO_BUILD_JOBS']='2'
results=[]
for i,name in enumerate(a.target or TARGETS):
    corpus=(folder/'corpus'/name) if a.fresh_corpus else (ROOT/'fuzz/corpus'/name)
    if a.fresh_corpus and corpus.exists():shutil.rmtree(corpus)
    corpus.mkdir(parents=True,exist_ok=True)
    for seed in (ROOT/'fuzz/seeds'/name).iterdir():
        if not (corpus/seed.name).exists():shutil.copyfile(seed,corpus/seed.name)
    log=folder/(name+'.log')
    limit={'scalars':4096,'patches':4096,'lineage':4096,'workbook_edits':2048,'twbx_structured':4096,'wire_frames':512,'edit_sequences':512}.get(name,32768)
    command=['cargo','+'+a.toolchain,'fuzz','run',name,str(corpus),'--','-max_total_time='+str(a.seconds),'-timeout=3','-rss_limit_mb=512','-max_len='+str(limit),'-print_final_stats=1','-use_value_profile=1','-seed='+str(a.seed_base+i)]
    start=time.monotonic()
    with log.open('wb') as out:
        process=subprocess.Popen(command,cwd=ROOT,env=env,stdout=out,stderr=subprocess.STDOUT,start_new_session=True)
        try:rc=process.wait(timeout=a.seconds+180)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid,signal.SIGKILL);process.wait();rc=124
    text=log.read_text(errors='replace')
    runs=re.findall(r'stat::number_of_executed_units:\s*(\d+)',text)
    cover=re.findall(r'cov:\s*(\d+)\s+ft:\s*(\d+)',text)
    item={'target':name,'exit_code':rc,'passed':rc==0,'seconds':round(time.monotonic()-start,2),'runs':int(runs[-1]) if runs else None,'coverage_edges':int(cover[-1][0]) if cover else None,'features':int(cover[-1][1]) if cover else None,'command':command,'log_sha256':hashlib.sha256(log.read_bytes()).hexdigest()}
    results.append(item);print(json.dumps(item),flush=True)
    (folder/'summary.json').write_text(json.dumps({'sanitizer':'address','toolchain':a.toolchain,'results':results},indent=2)+'\n')
raise SystemExit(0 if all(r['passed'] for r in results) else 1)
