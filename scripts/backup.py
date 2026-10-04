#!/usr/bin/env python3
"""Manual, local consistent backup bundle. No scheduled runs or automatic deletion."""
import argparse,pathlib,subprocess,shutil,os,json
p=argparse.ArgumentParser()
p.add_argument('--binary',default='target/release/threadbridge')
p.add_argument('--hub-db',required=True)
p.add_argument('--agent-db',action='append',default=[])
p.add_argument('--credential-file',action='append',default=[])
p.add_argument('--signing-dir',help='Optional protected APK signing directory; includes secrets')
p.add_argument('--destination',required=True)
a=p.parse_args();os.umask(0o077);dest=pathlib.Path(a.destination);dest.mkdir(parents=True,exist_ok=False)
manifest={'format':1,'contains_secrets':bool(a.credential_file or a.signing_dir),'databases':[]}
for i,source in enumerate([a.hub_db]+a.agent_db):
 name='hub.sqlite' if i==0 else f'agent-{i}.sqlite'
 subprocess.run([str(pathlib.Path(a.binary).resolve()),'backup','--db',source,'--output',str(dest/name)],check=True)
 manifest['databases'].append({'source':str(pathlib.Path(source).resolve()),'backup':name})
for i,source in enumerate(a.credential_file):shutil.copyfile(source,dest/f'credential-{i}.json')
if a.signing_dir:shutil.copytree(a.signing_dir,dest/'signing')
(dest/'manifest.json').write_text(json.dumps(manifest,indent=2))
print(f'Local backup complete: {dest}. Keep it in encrypted local storage; restore databases read-only before re-pairing.')
