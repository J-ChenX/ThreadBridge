#!/usr/bin/env python3
"""Release contract regressions: version drift, upgrade identity and build publication gate."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import release_policy as policy
import apk_metadata


class ReleasePolicyTests(unittest.TestCase):
    def setUp(self):
        self.p = policy.load_policy()
        self.prior = dict(package='dev.threadbridge', version=self.p['publishedVersionName'],
                          version_code=self.p['publishedVersionCode'], debuggable=False,
                          signer_sha256=self.p['signerSha256'], sha256=self.p['publishedApkSha256'])
        self.current = dict(self.prior, version=self.p['versionName'], version_code=self.p['versionCode'], sha256='b'*64)

    def test_valid_upgrade_and_historical_baseline(self):
        policy.validate_previous(self.prior, self.p)
        policy.validate_current(self.current, self.p, policy.artifact_name(self.p))
        self.assertGreater(policy.version(self.p['versionName']), policy.version(self.p['publishedVersionName']))

    def test_candidate_identity_version_and_filename_cannot_be_disguised(self):
        for changes in ({'package':'dev.threadbridge.debug'}, {'debuggable':True},
                        {'signer_sha256':'f'*64}, {'version':'0.1.21-test'},
                        {'version':self.p['publishedVersionName']}, {'version_code':self.p['publishedVersionCode']}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                policy.validate_current(dict(self.current, **changes), self.p, policy.artifact_name(self.p))
        with self.assertRaises(ValueError):
            policy.validate_current(self.current, self.p, 'ThreadBridge-questions-debug.apk')

    def test_wrong_or_older_published_apk_is_not_an_upgrade_baseline(self):
        for field, value in [('package','other'),('version','1.0.0'),('version_code',1),
                             ('signer_sha256','f'*64),('sha256','f'*64),('debuggable',True)]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                policy.validate_previous(dict(self.prior, **{field:value}), self.p)

    def test_version_policy_rejects_regression_suffixes_duplicates_and_missing_fields(self):
        original = policy.POLICY.read_text()
        variants = [original.replace('versionName='+self.p['versionName'], 'versionName='+value)
                    for value in (self.p['publishedVersionName'],'0.1.21-test','1.0.3-debug','01.0.3')]
        variants += [original.replace('versionCode='+str(self.p['versionCode']), 'versionCode='+str(self.p['publishedVersionCode'])),
                     original+'\nversionCode=999\n', original.replace('signerSha256=', 'unknown=')]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'release.properties'
            for text in variants:
                path.write_text(text)
                with self.subTest(text=text), self.assertRaises(ValueError):policy.load_policy(path)

    def test_inspection_checks_real_tool_results_and_debug_flag(self):
        with tempfile.TemporaryDirectory() as directory:
            apk=Path(directory)/'candidate.apk';apk.write_bytes(b'APK fixture')
            cert='Signer #1 certificate SHA-256 digest: '+'a'*64+'\n'
            badging="package: name='dev.threadbridge' versionCode='26' versionName='1.0.3'\napplication-debuggable\n"
            with mock.patch.object(apk_metadata.subprocess,'check_output',side_effect=[cert,badging]), mock.patch.object(apk_metadata.subprocess,'run') as align:
                result=apk_metadata.inspect(apk,Path(directory))
            self.assertTrue(result['debuggable']);self.assertEqual(result['sha256'],hashlib.sha256(b'APK fixture').hexdigest())
            self.assertTrue(align.call_args.kwargs['check'])
            for certificates in ('',cert+cert.replace('#1','#2')):
                with mock.patch.object(apk_metadata.subprocess,'check_output',return_value=certificates), self.assertRaises(ValueError):
                    apk_metadata.inspect(apk,Path(directory))

    def test_previous_apk_is_required_even_for_a_well_named_candidate(self):
        result=subprocess.run([sys.executable,ROOT/'scripts/verify-release.py','ThreadBridge-1.0.3.apk'],capture_output=True,text=True)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('--previous',result.stderr)


@unittest.skipUnless(os.name=='posix', 'The local release entrypoint is a Bash script')
class BuildPipelineTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)
        for directory in ('scripts','android','sdk/build-tools/35.0.0','artifacts','java/bin'):(self.root/directory).mkdir(parents=True)
        for name in ('build-apk.sh','verify-release.py','release_policy.py','apk_metadata.py'):
            shutil.copyfile(ROOT/'scripts'/name,self.root/'scripts'/name)
        self.baseline=self.root/'previous.apk';self.baseline.write_bytes(b'previous-release-fixture')
        self.p=policy.load_policy()
        self.p['publishedApkSha256']=hashlib.sha256(self.baseline.read_bytes()).hexdigest()
        self.write_policy()
        self.fixture={'package':'dev.threadbridge','debuggable':False,'signer':self.p['signerSha256'],
                      'version':self.p['versionName'],'code':self.p['versionCode'],
                      'prior_version':self.p['publishedVersionName'],'prior_code':self.p['publishedVersionCode'],
                      'prior_signer':self.p['signerSha256']}
        self.runner=self.root/'runner.py'
        self.runner.write_text('#!'+sys.executable+'\n'+'''import json,pathlib,sys
root=pathlib.Path(__file__).resolve().parent
config=json.loads((root/'fixture.json').read_text())
name=pathlib.Path(sys.argv[0]).name
if name=='gradlew':
 with (root/'calls').open('a') as f:f.write(' '.join(sys.argv[1:])+'\\n')
 if ':app:assembleRelease' in sys.argv:
  apk=root/'android/app/build/outputs/apk/release/app-release.apk';apk.parent.mkdir(parents=True,exist_ok=True);apk.write_bytes(b'candidate-release-fixture')
 sys.exit(config.get('gradle_exit',0) if ':app:assembleRelease' in sys.argv else config.get('jvm_exit',0))
apk=pathlib.Path(sys.argv[-1]);prior=apk.name=='previous.apk'
if name=='apksigner':
 print('Signer #1 certificate SHA-256 digest: '+(config['signer'] if not prior else config.get('prior_signer',config['signer'])))
elif name=='aapt':
 package=config['package'] if not prior else 'dev.threadbridge'
 version=config['prior_version'] if prior else config['version'];code=config['prior_code'] if prior else config['code']
 print(f"package: name='{package}' versionCode='{code}' versionName='{version}'")
 if config['debuggable'] and not prior:print('application-debuggable')
elif name=='zipalign':sys.exit(config.get('prior_align_exit',0) if prior else config.get('align_exit',0))
else:raise RuntimeError('unexpected fixture tool')
''')
        self.runner.chmod(0o755)
        for name in ('apksigner','aapt','zipalign'):(self.root/'sdk/build-tools/35.0.0'/name).symlink_to(self.runner)
        (self.root/'android/gradlew').symlink_to(self.runner)
        self.env=dict(os.environ,JAVA_HOME=str(self.root/'java'),ANDROID_HOME=str(self.root/'sdk'),
                      THREADBRIDGE_KEYSTORE=str(self.root/'fixture.jks'),THREADBRIDGE_KEY_PASSWORD='synthetic-only')

    def write_policy(self):
        (self.root/'android/release.properties').write_text('\n'.join(f'{k}={v}' for k,v in self.p.items())+'\n')

    def run_build(self):
        (self.root/'fixture.json').write_text(json.dumps(self.fixture))
        return subprocess.run(['bash',self.root/'scripts/build-apk.sh',self.baseline],cwd=self.root,env=self.env,capture_output=True,text=True)

    def test_success_requires_release_lint_upgrade_gate_and_complete_versioned_bundle(self):
        result=self.run_build();self.assertEqual(result.returncode,0,result.stderr)
        apk=self.root/'artifacts'/policy.artifact_name(self.p)
        self.assertTrue(apk.is_file());self.assertTrue(Path(str(apk)+'.sha256').is_file())
        report=json.loads(apk.with_suffix('.verification.json').read_text())
        self.assertTrue(report['delivery_approved']);self.assertTrue(report['upgrade_identity_verified'])
        self.assertEqual(report['sha256'],hashlib.sha256(apk.read_bytes()).hexdigest())
        self.assertEqual(Path(str(apk)+'.sha256').read_text().split(),[report['sha256'],apk.name])
        calls=(self.root/'calls').read_text();self.assertIn(':app:testDebugUnitTest',calls)
        self.assertIn(':app:assembleRelease :app:lintRelease',calls);self.assertNotIn(':app:assembleDebug',calls)
        before=apk.read_bytes();self.assertNotEqual(self.run_build().returncode,0);self.assertEqual(apk.read_bytes(),before)
        self.assertFalse(list((self.root/'artifacts').glob('.release-staging.*')))

    def test_failed_gates_never_publish_a_formal_apk(self):
        for changes in ({'debuggable':True},{'package':'dev.threadbridge.debug'},
                        {'code':self.p['publishedVersionCode']},{'version':'0.1.21-test'},
                        {'signer':'f'*64},{'align_exit':1},{'gradle_exit':1},{'jvm_exit':1}):
            with self.subTest(changes=changes):
                original=copy.deepcopy(self.fixture);self.fixture.update(changes)
                result=self.run_build();self.assertNotEqual(result.returncode,0)
                self.assertFalse(list((self.root/'artifacts').glob('*.apk')))
                self.assertFalse(list((self.root/'artifacts').glob('*.verification.json')))
                self.assertFalse(list((self.root/'artifacts').glob('.release-staging.*')))
                self.fixture=original

    def test_wrong_baseline_and_stale_policy_fail_before_gradle(self):
        self.fixture['prior_signer']='f'*64
        self.assertNotEqual(self.run_build().returncode,0);self.assertFalse((self.root/'calls').exists())
        self.fixture['prior_signer']=self.p['signerSha256'];self.p['versionCode']=self.p['publishedVersionCode'];self.write_policy()
        self.assertNotEqual(self.run_build().returncode,0);self.assertFalse((self.root/'calls').exists())


if __name__=='__main__':unittest.main(verbosity=2)
