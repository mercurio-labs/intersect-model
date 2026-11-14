"""Regenerate typed String phase evidence without rewriting the expected fixture."""
import argparse,gzip,hashlib,json,os,subprocess,traceback
from pathlib import Path

def pin(path):
 with Path(path).open('rb') as stream:digest=hashlib.file_digest(stream,'sha256').hexdigest()
 return {'bytes':Path(path).stat().st_size,'sha256':digest}

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('--out',required=True,type=Path,help='New output directory; never overwrites expected observations')
 args=parser.parse_args()
 root=Path(__file__).resolve().parent.parent
 fixture=root/'fixtures/rcore-input-string-transport'
 output=args.out.resolve();output.mkdir(parents=True,exist_ok=False)
 def save(name,value):(output/name).write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8',newline='\n')
 try:
  inputs={name:pin(fixture/name) for name in ('requests.jsonl','original-terminals.json','exact-plan.json','typed-phase-witness.json','recorded/shared-candidate-record.gz','native/src/driver.rs','typed-phase-witness.rs','typed-phase-native/Cargo.toml','typed-phase-native/Cargo.lock')}
  accepted=fixture/'recorded/shared-candidate-record.gz'
  assert pin(accepted)=={'bytes':26778,'sha256':'5e2697888991375a4decc48af207cc23b23088f6348252e2e04433525e0a135b'}
  raw=gzip.decompress(accepted.read_bytes())
  assert hashlib.sha256(raw).hexdigest()=='cd6b87588d1dc8e76765977951860d40d18c6155f9b1d6764c9a203e225220fd'
  accepted_copy=output/'accepted-retained-record.jsonl';accepted_copy.write_bytes(raw)
  command=['cargo','build','--manifest-path',str(fixture/'typed-phase-native/Cargo.toml'),'--locked','--offline','--message-format=json']
  save('preexecution.json',{'producer':pin(__file__),'inputs':inputs,'command':command,'gate_closures':[]})
  run=subprocess.run(command,cwd=root,capture_output=True)
  (output/'build.stdout').write_bytes(run.stdout);(output/'build.stderr').write_bytes(run.stderr)
  assert run.returncode==0,run.stderr.decode(errors='replace')
  executables=[json.loads(line)['executable'] for line in run.stdout.splitlines() if json.loads(line).get('reason')=='compiler-artifact' and json.loads(line).get('executable')]
  assert len(executables)==1
  captures=[]
  for index in range(2):
   run=subprocess.run([executables[0],str(fixture),str(accepted_copy)],cwd=root,capture_output=True)
   (output/f'capture-{index}.json').write_bytes(run.stdout);(output/f'capture-{index}.stderr').write_bytes(run.stderr)
   assert run.returncode==0,run.stderr.decode(errors='replace')
   captures.append(run.stdout)
  assert captures[0]==captures[1]
  observation=json.loads(captures[0])
  compact={key:observation[key] for key in ('version','original_controls','copy_controls','scope','accepted_retained_record_sha256')}
  compact['source_records']=[{key:value for key,value in row.items() if key!='pending_rows'} for row in observation['source_records']]
  save('compact-witness.json',compact)
  assert compact==json.loads((fixture/'typed-phase-witness.json').read_bytes()),'Fresh whole observations differ; preserve this attempt and inspect, never normalize or rebase'
  assert all(pin(fixture/name)==expected for name,expected in inputs.items())
  save('receipt.json',{'state':'two_fresh_typed_phase_captures_match_complete_expected_fixture','inputs':inputs,'executable':{'path':executables[0],**pin(executables[0])},'captures':[pin(output/f'capture-{index}.json') for index in range(2)],'compact_witness':pin(output/'compact-witness.json'),'scope':'Original282requests retained;48fixed-schedule observations and48additional source-witnessed copy-prefix controls. No runtime or expected-fixture edits.','gate_closures':[]})
  print(json.dumps({'receipt':str(output/'receipt.json'),**pin(output/'receipt.json')}),flush=True)
 except BaseException:
  save('failure.json',{'state':'original_failed_attempt_preserved','traceback':traceback.format_exc()})
  raise

if __name__=='__main__':main()
