"""Serial diagnostic runs; allocation-instrumented timings are not parity results."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[2]

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--sizes', type=int, nargs='+', default=[500,1000,2000,4000])
    p.add_argument('--repeats', type=int, default=2)
    p.add_argument('--binary', type=Path, default=ROOT / 'target/release/examples' /
                   ('collection_growth_metrics.exe' if os.name == 'nt' else 'collection_growth_metrics'),
                   help='Explicit diagnostic executable (defaults to the release build)')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    binary = a.binary.resolve()
    rows = []
    for n in a.sizes:
        for repeat in range(a.repeats):
            for mode in (['append','bulk'] if repeat % 2 == 0 else ['bulk','append']):
                command = [str(binary),str(n),mode]
                result = subprocess.run(command,cwd=ROOT,capture_output=True,text=True,timeout=60)
                name = f'n{n}-{mode}-r{repeat}'
                (a.output/(name+'.stdout.log')).write_text(result.stdout,encoding='utf-8')
                (a.output/(name+'.stderr.log')).write_text(result.stderr,encoding='utf-8')
                result.check_returncode()
                row = json.loads(result.stdout)
                assert row['items']==row['verified_values']==n and row['rollback_verified']
                rows.append({**row,'repeat':repeat,'command':command})
                print(n,mode,round(row['instrumented_ms'],2),row['operation_retained_requested_bytes'],flush=True)
    report = {'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
              'source_status':subprocess.check_output(['git','status','--short'],cwd=ROOT,text=True),
              'binary_sha256':sha(binary),
              'harness_files':{str(path.relative_to(ROOT)).replace('\\','/'):sha(path) for path in [Path(__file__),ROOT/'crates/intersect-model/examples/collection_growth_metrics.rs']},
              'toolchain':subprocess.check_output(['cargo','--version'],cwd=ROOT,text=True).strip(),
              'runs':rows,'summary':[]}
    for n in a.sizes:
        for mode in ['append','bulk']:
            subset=[r for r in rows if r['items']==n and r['mode']==mode]
            report['summary'].append({'items':n,'mode':mode,**{key:statistics.median(r[key] for r in subset) for key in ['instrumented_ms','operation_retained_requested_bytes','operation_peak_requested_bytes','operation_cumulative_requested_bytes','allocation_calls']}})
    (a.output/'results.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print('COMPLETE',a.output/'results.json')

if __name__=='__main__':
    main()
