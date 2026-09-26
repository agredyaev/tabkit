#!/usr/bin/env python3
"""Small fault-injection campaign: prove selected critical regressions fail when guards are broken.
Mutates an isolated temporary source copy, never the working tree or live services.
A compile failure is NOT counted as a detected mutation.
"""
import hashlib,json,os,pathlib,shutil,subprocess,tempfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
REPORT=ROOT/'reports/testing-0.1.5/mutations';REPORT.mkdir(parents=True,exist_ok=True)
MUTANTS=[
 ('stale-source','src/edit.rs','require(changes.input_sha256 == package_sha256,','require(true,','regression::stale_input_hash_is_rejected'),
 ('dependency-closure','src/edit.rs','after.require_calculation_dependencies(&changed_calculations)?;','let _ = &changed_calculations;','regression::new_reference_missing_from_sheet_is_refused'),
 ('plan-integrity','src/app.rs','require(supplied==rebuilt,','require(true,','behavioral::tampered_patches_are_recomputed_even_with_new_external_hash'),
 ('duplicate-json','src/wire.rs','if !keys.insert(key){return Err(de::Error::custom("duplicate JSON object key"));}','keys.insert(key);','regression::lexical_admission_rejects_decoded_duplicate_keys'),
 ('patch-budget','src/patch.rs','require(length <= max && length <= usize::MAX as u64,','require(true,','regression::patch_budget_checked_before_candidate_allocation'),
 ('both-output-guards','src/fs.rs','require(!path.exists(), "OUTPUT_EXISTS", "Output exists; choose a new candidate path")?;','// fault injection: admission guard removed','behavioral::apply_refuses_to_replace_existing_output'),
 ('output-no-clobber','src/fs.rs','require(!path.exists(), "OUTPUT_EXISTS", "Output exists; choose a new candidate path")?;','// fault injection: premature admission of an existing file','behavioral::apply_refuses_to_replace_existing_output'),
]
# The no-clobber mutant should still be blocked by the independent atomic write
# boundary; this campaign verifies the public error contract, not arbitrary safety.
files=[p for p in ROOT.glob('src/*.rs')]+[ROOT/'Cargo.toml',ROOT/'Cargo.lock']
before={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
results=[]
with tempfile.TemporaryDirectory(prefix='tabkit-mutants-') as td:
    work=pathlib.Path(td)
    for folder in ['src','examples','resources','tests','native']:shutil.copytree(ROOT/folder,work/folder)
    for name in ['Cargo.toml','Cargo.lock','build.rs','rust-toolchain.toml']:shutil.copy2(ROOT/name,work/name)
    env=os.environ.copy();env['CARGO_BUILD_JOBS']='2';env['CARGO_TARGET_DIR']=str(ROOT/'target/mutation-tests')
    baseline=subprocess.run(['cargo','test','--locked'],cwd=work,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=240)
    (REPORT/'baseline.log').write_bytes(baseline.stdout)
    if baseline.returncode:raise SystemExit('Mutation baseline failed; no mutation scores valid')
    for name,path,old,new,test in MUTANTS:
        target=work/path;original=target.read_text()
        if original.count(old)!=1:raise AssertionError('Mutation site changed: '+name)
        package=work/'src/package.rs';package_original=package.read_text()
        try:
            target.write_text(original.replace(old,new,1))
            if name=='both-output-guards':
                assert 'tmp.persist_noclobber(output)' in package_original
                package.write_text(package_original.replace('tmp.persist_noclobber(output)','tmp.persist(output)',1))
            run=subprocess.run(['cargo','test','--locked',test,'--','--exact'],cwd=work,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=120)
            text=run.stdout.decode(errors='replace');(REPORT/(name+'.log')).write_bytes(run.stdout)
            detected=run.returncode==101 and 'test result: FAILED.' in text and '1 failed;' in text
            results.append({'mutation':name,'test':test,'detected_by_test':detected,'exit_code':run.returncode,'protected_by_independent_guard':name=='output-no-clobber' and run.returncode==0,'scope':'selected guard/error contract; both-output-guards deliberately disables two independent defenses; not exhaustive mutation score'})
        finally:
            target.write_text(original);package.write_text(package_original)
        print(json.dumps(results[-1]),flush=True)
        (REPORT/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
after={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
assert before==after,'Working tree was unexpectedly modified'
raise SystemExit(0 if all(r['detected_by_test'] or r['protected_by_independent_guard'] for r in results) else 1)
