"""Run real Model snapshots in separate write/restore processes; no backend needed."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import traceback
import uuid

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target-dir', default=os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target')))
    parser.add_argument('--profile', default='test-fast')
    parser.add_argument('--no-build', action='store_true')
    args = parser.parse_args()
    args.target_dir = str(Path(args.target_dir).resolve())
    evidence = ROOT/'target/snapshot-harness'/(time.strftime('%Y%m%d-%H%M%S')+'-'+uuid.uuid4().hex[:6])
    evidence.mkdir(parents=True)
    data = Path(tempfile.mkdtemp(prefix='intersect-model-snapshots-'))
    report = {'passed':False,'baseCommit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
              'dataDirectory':str(data),'profile':args.profile,
              'optLevelOverride':os.environ.get('CARGO_PROFILE_TEST_FAST_OPT_LEVEL'),
              'rustVersion':subprocess.check_output(['rustc','--version'],cwd=ROOT,text=True).strip(),
              'steps':[]}
    files = [ROOT/'Cargo.toml',ROOT/'Cargo.lock',ROOT/'crates/intersect-model/Cargo.toml',Path(__file__).resolve(),
             ROOT/'crates/intersect-model/examples/model_snapshots.rs',ROOT/'crates/intersect-model/tests/model_snapshots.rs',
             ROOT/'crates/intersect-model/assets/fixtures/snapshots/node.rcore']
    files.extend(sorted((ROOT/'crates/intersect-model/src').rglob('*.rs')))
    report['sourceFiles']={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
    try:
        if not args.no_build:
            command=['cargo','build','--profile',args.profile,'-p','intersect-model','--example','model_snapshots',
                     '--locked','--offline','--target-dir',args.target_dir,'-j','2']
            with (evidence/'build.log').open('w',encoding='utf-8') as log:
                subprocess.run(command,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,check=True,timeout=540)
        profile_directory = {'dev':'debug','release':'release'}.get(args.profile,args.profile)
        binary=Path(args.target_dir)/profile_directory/'examples'/('model_snapshots.exe' if os.name=='nt' else 'model_snapshots')
        report.update(binary=str(binary),binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        for action in ['--write','--verify']:
            process=subprocess.run([str(binary),action,str(data)],cwd=ROOT,capture_output=True,text=True,encoding='utf-8',check=True,timeout=60)
            (evidence/(action[2:]+'.json')).write_text(process.stdout,encoding='utf-8')
            result=json.loads(process.stdout)
            if not result['passed']: raise RuntimeError('Snapshot scenario failed')
            report['steps'].append({'action':action,'pidIsolated':True,'result':result})
        if report['steps'][0]['result']!=report['steps'][1]['result']:
            raise RuntimeError('Write and independent restore observations differ')
        report['artifacts']={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(data.glob('*.json'))}
        report['passed']=True
    except Exception:
        report['error']=traceback.format_exc()
        print(report['error'])
    finally:
        (evidence/'summary.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
        print('Evidence: '+str(evidence))
    return 0 if report['passed'] else 1


if __name__=='__main__':
    raise SystemExit(main())
