"""Replay archived pinned sources through sync and cold/warm review, without models.

--cases is a JSON array of {name, source_store, request}; request names a JSON
file accepted by `graphicle review`. Each case uses one repository per side and
at most 5,000 paths per side. The original stores are opened read-only. Output
must be a new directory and contains private scratch stores and raw packets.
Optional empty_seed names a previously prepared empty scratch-store template.

Run matching release binaries with no concurrent builds/test workloads. Cold
means no Graphicle analysis/package caches, not cold OS caches. Preparation,
server lifecycle and dependency collection are outside reported sync+review
stage totals. wait4 measures command peak RSS; phase logs also report Graphicle's
own high-water RSS. This tool currently requires a Unix host.
"""
import argparse, base64, hashlib, json, os, pathlib, re, shutil, signal, sqlite3, subprocess, sys, time

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT/'sdk/python'))
from graphicle_sdk import LocalServer

PHASE = re.compile(r'^phase=(\S+) ms=(\d+).*?peak_rss_mb=([\d.]+)')
def save(path, value):
    path.write_text(json.dumps(value, indent=2)+'\n')
def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()
def canonical(packet):
    return {k:v for k,v in packet.items() if k not in ('analysis_cache','tokens')}
def phases(log):
    totals={}; peak=0
    for line in log.splitlines():
        m=PHASE.match(line)
        if m:
            totals[m[1]]=totals.get(m[1],0)+int(m[2]);peak=max(peak,float(m[3]))
    return totals,peak

def prepare(case, root):
    source=pathlib.Path(case['source_store']).resolve()
    request=json.loads(pathlib.Path(case['request']).read_text())
    repository=request['repository_id']
    original=sqlite3.connect((source/'reviews.sqlite3').as_uri()+'?mode=ro',uri=True)
    sides={}
    for side in ('base','head'):
        assert len(request[side])==1, 'This replay fixture supports one repository per side'
        pin=request[side][0]
        assert pin['repository_id']==repository
        row=original.execute('SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?',(repository,pin['snapshot'])).fetchone()
        manifest,metadata=map(json.loads,row)
        # Prefer the exact commit/policy binding over legacy snapshot metadata.
        tables={r[0] for r in original.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if 'repo_snapshot_bindings' in tables:
            columns={r[1] for r in original.execute('PRAGMA table_info(repo_snapshot_bindings)')}
            if 'withheld_paths' in columns:
                binding=original.execute('SELECT scope_complete,withheld_paths FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?',(repository,pin['snapshot'],pin['commit'],pin['policy_sha256'])).fetchone()
                if binding:
                    metadata['scope_complete']=bool(binding[0])
                    if binding[1] is not None:metadata['withheld_paths']=json.loads(binding[1])
        metadata.update(commit=pin['commit'],policy_sha256=pin['policy_sha256'])
        unique={v['sha256']:v['bytes'] for v in manifest.values()}
        for digest,size in unique.items():
            row=original.execute('SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?',(repository,digest)).fetchone()
            assert row is not None,(case['name'],'missing object',digest)
            raw=row[0].encode() if isinstance(row[0],str) else row[0]
            assert len(raw)==size and hashlib.sha256(raw).hexdigest()==digest
        sides[side]={'manifest':manifest,'metadata':metadata,'files':len(manifest),'bytes':sum(v['bytes'] for v in manifest.values()),'unique_objects':len(unique)}
    seed=root/case['name']/'seed';seed.mkdir(parents=True)
    if case.get('empty_seed'):
        shutil.copytree(case['empty_seed'],seed,dirs_exist_ok=True)
        with sqlite3.connect(seed/'reviews.sqlite3') as check:
            for table in ['repo_objects','repo_snapshots','repo_review_analysis']:
                assert check.execute('SELECT count(*) FROM '+table).fetchone()[0]==0
            assert check.execute('SELECT snapshot FROM repo_projects WHERE id=?',(repository,)).fetchone()==('',)
        original.close()
        save(root/case['name']/'request.json',request)
        return sides,seed,request
    db=sqlite3.connect(seed/'reviews.sqlite3')
    original.backup(db)
    db.execute('PRAGMA foreign_keys=ON')
    columns=[r[1] for r in db.execute('PRAGMA table_info(repo_projects)')]
    project=dict(zip(columns,db.execute('SELECT * FROM repo_projects WHERE id=?',(repository,)).fetchone()))
    project.update(snapshot='',generation=0)
    db.execute('DELETE FROM repo_projects')
    db.execute('INSERT INTO repo_projects ('+','.join(columns)+') VALUES ('+','.join('?' for _ in columns)+')',[project[c] for c in columns])
    db.execute("INSERT INTO repo_search(repo_search) VALUES ('rebuild')")
    db.commit();db.execute('VACUUM');db.close();original.close()
    shutil.copyfile(source/'local-session.json',seed/'local-session.json')
    save(root/case['name']/'request.json',request)
    return sides,seed,request

def sync(binary,work,source,request,sides,config,env):
    repo=request['repository_id'];source_db=sqlite3.connect((source/'reviews.sqlite3').as_uri()+'?mode=ro',uri=True)
    server=LocalServer(binary,work/'store',config,env=env,timeout=600,startup_timeout=120)
    summaries={};began=None
    try:
        with server as client:
            expected='';began=time.monotonic()
            for side in ('base','head'):
                stage=time.monotonic();manifest=sides[side]['manifest']
                prep_args={'repository_id':repo,'expected_snapshot':expected,'metadata':sides[side]['metadata'],'missing_limit':256}
                entries=[{'path':p,**v} for p,v in manifest.items()]
                assert len(entries)<=5000
                start=time.monotonic();page=client.command('sync.prepare',manifest=entries,**prep_args);prepare_seconds=time.monotonic()-start
                assert page['snapshot']==request[side][0]['snapshot']
                sync_id=page['sync_id'];missing=list(page['missing']);pages=1;discovery=prepare_seconds
                while page.get('next_cursor'):
                    start=time.monotonic();page=client.command('sync.missing',repository_id=repo,sync_id=sync_id,cursor=page['next_cursor'],missing_limit=256);discovery+=time.monotonic()-start
                    missing.extend(page['missing']);pages+=1
                start=time.monotonic();batch=[];batch_bytes=0;uploads=0
                def upload():
                    nonlocal batch,batch_bytes,uploads
                    if batch:
                        client.command('sync.upload',repository_id=repo,sync_id=sync_id,files=batch);uploads+=1;batch=[];batch_bytes=0
                for path in missing:
                    raw=source_db.execute('SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?',(repo,manifest[path]['sha256'])).fetchone()[0]
                    raw=raw.encode() if isinstance(raw,str) else raw
                    # Bound ordinary uploads; individual large objects use the local HTTP body's limit.
                    if batch and (batch_bytes+len(raw)>512*1024 or len(batch)>=256):upload()
                    batch.append({'path':path,'content_base64':base64.b64encode(raw).decode()});batch_bytes+=len(raw)
                upload();upload_seconds=time.monotonic()-start
                start=time.monotonic();receipt=client.command('sync.commit',repository_id=repo,sync_id=sync_id,compact=True);commit_seconds=time.monotonic()-start
                expected=request[side][0]['snapshot']
                summaries[side]={'seconds':time.monotonic()-stage,'prepare_and_missing_seconds':discovery,'upload_seconds':upload_seconds,'commit_seconds':commit_seconds,'pages':pages,'uploaded_objects':len(missing),'upload_calls':uploads,'receipt':receipt}
                print(work.name,side,'sync',round(summaries[side]['seconds'],3),flush=True)
            elapsed=time.monotonic()-began
            assert client.usage()['model_usage']['calls']==0
    finally:
        source_db.close()
        (work/'sync-phases.log').write_text('\n'.join(x for x in server.logs().splitlines() if x.startswith('phase='))+'\n')
    return {'seconds':elapsed,'sides':summaries}

def review(binary,work,request_path,mode,config,env):
    start=time.monotonic()
    with (work/(mode+'.json')).open('w') as out,(work/(mode+'.log')).open('w') as log:
        process=subprocess.Popen([str(binary),'--data-dir',str(work/'store'),'--config',str(config),'review','--request',str(request_path)],stdout=out,stderr=log,env=env,cwd=work,start_new_session=True)
        while True:
            waited,status,usage=os.wait4(process.pid,os.WNOHANG)
            if waited:
                process.returncode=os.waitstatus_to_exitcode(status)
                break
            if time.monotonic()-start>600:
                os.killpg(process.pid,signal.SIGKILL)
                _,status,usage=os.wait4(process.pid,0)
                process.returncode=os.waitstatus_to_exitcode(status)
                raise TimeoutError(f'{work.name} {mode} exceeded 600 seconds')
            time.sleep(0.02)
    elapsed=time.monotonic()-start
    assert process.returncode==0,(work,mode,'see private log')
    packet=json.loads((work/(mode+'.json')).read_text());log=(work/(mode+'.log')).read_text();phase_ms,phase_peak=phases(log)
    peak_bytes=usage.ru_maxrss if sys.platform=='darwin' else usage.ru_maxrss*1024
    return packet,{'seconds':elapsed,'phase_ms':phase_ms,'phase_peak_rss_mib':phase_peak,'process_peak_rss_bytes':peak_bytes,'analysis_cache':packet['analysis_cache'],'entries':len(packet['entries']),'impact':packet['impact'],'issue_count':packet['coverage']['issue_count']}

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--before',type=pathlib.Path,required=True);p.add_argument('--after',type=pathlib.Path,required=True)
    p.add_argument('--cases',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True)
    args=p.parse_args();os.umask(0o077);root=args.output.resolve();root.mkdir(parents=True,exist_ok=False)
    cases=json.loads(args.cases.read_text());binaries={'before':args.before.resolve(),'after':args.after.resolve()}
    config=root/'models.json';save(config,{'models':{}})
    summary={'binaries':{k:{'path':str(v),'sha256':sha(v)} for k,v in binaries.items()},'method':'Sequential ABBA; fresh sync then cold and warm review per arm; cold means no Graphicle analysis/package caches, not cold OS cache; local HTTP sync and CLI review; no models or dependency collection','cases':{},'rows':[]}
    for case in cases:
        assert re.fullmatch(r'[A-Za-z0-9_-]+',case['name']), 'Use a simple case name'
    assert len({case['name'] for case in cases})==len(cases), 'Duplicate case name'
    prepared={}
    for case in cases:
        sides,seed,request=prepare(case,root);prepared[case['name']]=(sides,seed,request)
        summary['cases'][case['name']]={'source_store':case['source_store'],'request_sha256':sha(root/case['name']/'request.json'),'request':request,'sides':{s:{k:v for k,v in data.items() if k not in ('manifest','metadata')} for s,data in sides.items()}}
    save(root/'summary.json',summary)
    reference={}
    for index,arm in enumerate(['before','after','after','before']):
        for case in cases:
            name=case['name'];sides,seed,request=prepared[name];work=root/name/f'{index}-{arm}';work.mkdir()
            shutil.copytree(seed,work/'store')
            env={'PATH':os.environ['PATH'],'HOME':str(work),'TMPDIR':'/private/tmp','GOENV':'off','GOWORK':'off','GOTOOLCHAIN':'local','GOPROXY':'off','GOSUMDB':'off','GOVCS':'*:off','CGO_ENABLED':'0','GRAPHICLE_PHASE_TIMINGS':'1','RUST_MIN_STACK':'16777216'}
            row={'case':name,'arm':arm,'order':index+1,'load_average_before':os.getloadavg()}
            row['sync']=sync(binaries[arm],work,pathlib.Path(case['source_store']),request,sides,config,env)
            for mode in ['cold','warm']:
                packet,stats=review(binaries[arm],work,root/name/'request.json',mode,config,env)
                row[mode]=stats
                if mode=='cold':assert not packet['analysis_cache']['base_hit'] and not packet['analysis_cache']['head_hit'],(name,mode,'unexpected cache state')
                canonical_packet=canonical(packet)
                if name not in reference:reference[name]=canonical_packet
                equal=canonical_packet==reference[name];row[mode]['packet_equal']=equal
                save(root/'summary.json',{**summary,'active':row})
                assert equal,(name,arm,mode,'packet drift')
                print(name,arm,mode,json.dumps(stats),flush=True)
            row['sync_plus_cold_seconds']=row['sync']['seconds']+row['cold']['seconds']
            summary['rows'].append(row);save(root/'summary.json',summary)
    summary['complete']=True;save(root/'summary.json',summary)
if __name__=='__main__':main()
