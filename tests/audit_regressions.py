#!/usr/bin/env python3
"""macOS end-to-end regressions for the validated audit findings.

Run: python3 tests/audit_regressions.py target/debug/acre
Requires Git, Bash, Zsh, Node/npm and gh. All repositories and homes are disposable;
GitHub metadata is stubbed and the real gh routing probe cannot forward traffic.
AUDIT_FILTER selects comma-separated test names. Failure transcripts are retained.
"""
import json
import http.server
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import threading
import traceback

BASE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else BASE.parent/'target/debug/acre').resolve()
RUN = Path(tempfile.mkdtemp(prefix='acre-regressions-')).resolve()
ENV = dict(os.environ)
for key in list(ENV):
    if key.startswith('ACRE_') or key.startswith('GIT_'):
        ENV.pop(key)
ENV.update(PATH='/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:'+os.environ.get('PATH',''), GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1', GIT_TERMINAL_PROMPT='0')

class Fixture:
    def __init__(self, name, settings=None, files=None):
        self.root = RUN / name
        self.root.mkdir()
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.config = self.root / 'config.json'
        self.settings = {'root': str(self.root / 'acre'), 'pool': {'minSlots': 1, 'maxSlots': 1, 'replenish': False}}
        if settings:
            self.settings.update(settings)
        self.config.write_text(json.dumps(self.settings))
        self.env = dict(ENV, ACRE_CONFIG=str(self.config))
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'audit@example.invalid')
        self.git('config', 'user.name', 'Audit')
        defaults = {'.gitignore': 'node_modules/\n.env\n.env.local\nlocal/\n', 'package.json': '{"name":"fixture"}\n', 'readme.txt': 'initial\n'}
        for key, value in (files if files is not None else defaults).items():
            self.write(key, value)
        self.commit()

    def write(self, path, content, root=None):
        target = (root or self.repo) / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        return target

    def cmd(self, argv, cwd=None, env=None, check=True):
        p = subprocess.run(list(map(str, argv)), cwd=cwd or self.repo, env=env or self.env, capture_output=True, text=True, timeout=45)
        with (self.root / 'trace.jsonl').open('a') as log:
            log.write(json.dumps({'argv': list(map(str, argv)), 'cwd': str(cwd or self.repo), 'code': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr}) + '\n')
        if check and p.returncode:
            raise AssertionError((argv, p.returncode, p.stdout, p.stderr))
        return p

    def git(self, *args, cwd=None, check=True):
        return self.cmd(['/usr/bin/git', *args], cwd=cwd, check=check)

    def commit(self, root=None):
        self.git('add', '-A', cwd=root)
        self.git('commit', '-qm', 'fixture change', cwd=root)

    def acre(self, *args, cwd=None, env=None, check=True):
        return self.cmd([BINARY, '--json', *args], cwd=cwd, env=env, check=check)

    def data(self, *args, **kwargs):
        return json.loads(self.acre(*args, **kwargs).stdout)

    def new(self, branch='work', **kwargs):
        return self.data('new', branch, '--stay', **kwargs)

    def shell(self, *args, cwd=None, session='audit-session'):
        directive = self.root / 'directive'
        directive.write_bytes(b'')
        env = dict(self.env, ACRE_SHELL_SESSION_ID=session, ACRE_SHELL_PID=str(os.getpid()), ACRE_DIRECTIVE_FILE=str(directive))
        p = self.cmd([BINARY, *args], cwd=cwd, env=env, check=False)
        return p, directive.read_bytes().split(b'\0'), env

def test_failed_remote_activation_preserves_existing_branch():
    f = Fixture('remote-rollback')
    f.git('remote', 'add', 'origin', str(f.repo))
    f.git('branch', 'feature')
    f.git('update-ref', 'refs/remotes/origin/feature', 'HEAD')
    before = f.git('show-ref', '--verify', 'refs/heads/feature').stdout.strip()
    result = f.acre('remote:origin/feature', check=False)
    after = f.git('show-ref', '--verify', 'refs/heads/feature', check=False)
    assert result.returncode != 0 and after.returncode == 0 and after.stdout.strip() == before
    return

def test_branch_selectors_follow_current_git_state():
    f = Fixture('stale-branch')
    path = Path(f.new('old')['path'])
    f.git('switch', '-c', 'different', cwd=path)
    reopened = f.data('old')
    actual = f.git('branch', '--show-current', cwd=Path(reopened['path'])).stdout.strip()
    assert reopened['branch'] == 'old' and actual == 'old'
    assert f.git('branch','--show-current',cwd=path).stdout.strip() == 'different'
    return

def test_ignored_seed_siblings_are_preserved():
    f = Fixture('seed-ancestor', {'environment': {'seedFiles': ['local/.env']}})
    f.write('local/.env', 'ORIGINAL_SECRET=fixture\n')
    path = Path(f.new()['path'])
    assert (path / 'local/.env').exists()
    notes = f.write('local/unique-notes.txt', 'only copy of user work', root=path)
    f.data('system', 'warm', '--slots', '1')
    result = f.data('done', 'work', check=False)
    assert not result['ok'] and notes.read_text() == 'only copy of user work'
    return


def test_nested_projects_receive_dependencies():
    f = Fixture('nested-ecosystem', files={'.gitignore': 'node_modules/\n', 'apps/web/package.json': '{"dependencies":{"left-pad":"1.3.0"}}\n'})
    f.write('apps/web/node_modules/present', 'installed dependencies')
    created = f.new()
    assert 'node_modules' in created['environment']['cacheRoots'] and created['environment']['state'] == 'ready'
    assert (Path(created['path'])/'apps/web/node_modules/present').read_text() == 'installed dependencies'
    return

def test_pnpm_workspace_changes_invalidate_pooled_caches():
    f = Fixture('workspace-fingerprint')
    f.write('pnpm-lock.yaml', 'lockfileVersion: 9.0\n')
    f.write('pnpm-workspace.yaml', 'packages:\n  - packages/a\nonlyBuiltDependencies: []\n')
    f.commit()
    f.write('node_modules/version', 'old workspace setup')
    a = f.new()
    f.data('done', 'work')
    f.write('pnpm-workspace.yaml', 'packages:\n  - packages/b\nonlyBuiltDependencies:\n  - esbuild\n')
    f.commit()
    # Only the pooled old generation remains available; a primary cache with unknown
    # installation health must not obscure whether that generation was invalidated.
    shutil.rmtree(f.repo/'node_modules')
    b = f.new('next')
    assert a['environment']['fingerprint'] != b['environment']['fingerprint']
    assert b['environment']['state'] == 'cold' and not b['reused']
    return

def test_remote_name_changes_preserve_leases():
    f = Fixture('repository-name')
    acquired = f.data('acquire', 'work', '--new', '--holder', 'audit')
    f.git('remote', 'add', 'origin', 'https://github.com/example/different-name.git')
    inspected = f.data('system', 'inspect')
    result = f.acre('release', '--lease-id', acquired['lease_id'], check=False)
    assert len(inspected['state']['workspaces']) == 1 and result.returncode == 0
    return

def test_previous_location_preserves_actual_subdirectory():
    f = Fixture('previous-location')
    f.write('src/nested/file', 'x')
    f.commit()
    p, directive, env = f.shell('new', 'work')
    assert p.returncode == 0
    path = Path(directive[2].decode())
    subdir = path/'src/nested'
    p, directive, env = f.shell('main', cwd=subdir)
    assert p.returncode == 0
    p, directive, env = f.shell('-', cwd=Path(directive[2].decode()))
    actual = Path(directive[2].decode())
    assert actual == subdir
    return

def test_previous_json_does_not_navigate():
    f = Fixture('previous-json')
    p, directive, env = f.shell('new', 'work')
    path = Path(directive[2].decode())
    p, directive, env = f.shell('--json', '-', cwd=path)
    assert p.returncode == 0 and directive == [b'']
    assert json.loads(p.stdout)['navigated'] is False



def test_foreign_pr_urls_are_rejected():
    f = Fixture('pr-url')
    f.git('remote', 'add', 'origin', 'https://github.com/current/project.git')
    oid = f.git('rev-parse', 'HEAD').stdout.strip()
    binpath = f.root/'bin'
    binpath.mkdir()
    reply = {'number': 7, 'title': 'Current repository PR', 'author': {'login': 'audit'}, 'url': 'https://github.com/current/project/pull/7', 'baseRefName':'main', 'headRefName':'head', 'headRefOid':oid, 'isCrossRepository':False, 'headRepository':{'nameWithOwner':'current/project'}}
    fake = binpath/'gh'
    fake.write_text('#!/usr/bin/python3\nimport json,sys\nfrom pathlib import Path\nPath('+repr(str(f.root/'gh-args.json'))+').write_text(json.dumps(sys.argv[1:]))\nprint('+repr(json.dumps(reply))+')\n')
    fake.chmod(0o755)
    env = dict(f.env, PATH=str(binpath)+':'+f.env['PATH'])
    result = f.data('https://github.com/other/unrelated/pull/7', env=env, check=False)
    assert result['error']['code'] == 'ACRE_PR_REPOSITORY_MISMATCH'
    assert not (f.root/'gh-args.json').exists()


def test_pull_request_urls_allow_pull_as_a_repository_name():
    for owner, repo in [('pull', 'project'), ('current', 'pull')]:
        f = Fixture('pr-url-' + owner + '-' + repo)
        f.git('remote', 'add', 'origin', 'https://github.com/' + owner + '/' + repo + '.git')
        oid = f.git('rev-parse', 'HEAD').stdout.strip()
        url = 'https://github.com/' + owner + '/' + repo + '/pull/7'
        bindir = f.root/'bin'
        bindir.mkdir()
        reply = {'number': 7, 'title': 'Pull name', 'url': url, 'baseRefName': 'main',
                 'headRefName': 'feature', 'headRefOid': oid, 'isCrossRepository': False}
        gh = bindir/'gh'
        gh.write_text('#!/usr/bin/python3\nprint(' + repr(json.dumps(reply)) + ')\n')
        gh.chmod(0o755)
        env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'])
        result = f.data(url, env=env)
        assert result['ok'] and result['target_kind'] == 'pull-request', result

def test_package_renames_invalidate_old_workspace_links():
    f = Fixture('package-name', files={'.gitignore':'node_modules/\n', 'package.json':'{"name":"root","private":true,"workspaces":["packages/*"]}\n', 'packages/a/package.json':'{"name":"old-name","version":"1.0.0","main":"index.js"}\n', 'packages/a/index.js':'module.exports = 42;\n'})
    f.cmd([shutil.which('npm', path=ENV['PATH']), 'install', '--ignore-scripts', '--no-package-lock', '--offline', '--no-audit', '--no-fund', '--cache', f.root/'npm-cache'])
    assert (f.repo/'node_modules/old-name').is_symlink()
    a = f.new()
    f.data('done', 'work')
    f.write('packages/a/package.json', '{"name":"new-name","version":"1.0.0","main":"index.js"}\n')
    f.commit()
    # Refresh the trusted primary installation. Readiness does not validate installed packages;
    # this regression isolates selecting an old pooled generation over a correct new source.
    f.cmd([shutil.which('npm', path=ENV['PATH']), 'install', '--ignore-scripts', '--no-package-lock', '--offline', '--no-audit', '--no-fund', '--cache', f.root/'npm-cache'])
    b = f.new('next')
    result = f.cmd([shutil.which('node', path=ENV['PATH']), '-p', 'require("new-name")'], cwd=Path(b['path']), check=False)
    assert a['environment']['fingerprint'] != b['environment']['fingerprint']
    assert not b['reused'] and result.returncode == 0 and result.stdout.strip() == '42'
    assert not (Path(b['path'])/'node_modules/old-name').exists()
    return

def test_shell_initialization_is_repeatable():
    evidence = {}
    for shell in ['/bin/bash', '/bin/zsh']:
        f = Fixture('reload-'+Path(shell).name)
        bindir = f.root/'bin'
        bindir.mkdir()
        (bindir/'acre').symlink_to(BINARY)
        env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'])
        kind = Path(shell).name
        script = 'eval "$(acre shell init '+kind+')"\neval "$(acre shell init '+kind+')"\nacre --version\n'
        # No startup files: the developer's own rc files are not part of what is being tested.
        isolated = ['--noprofile', '--norc'] if kind == 'bash' else ['-f']
        p = f.cmd([shell, *isolated, '-c', script], env=env, check=False)
        assert p.returncode == 0 and 'acre ' in p.stdout
        evidence[kind] = {'code':p.returncode,'stderr':p.stderr}
    return

def test_child_shells_have_distinct_sessions():
    f = Fixture('inherited-shell-session')
    env = dict(f.env, ACRE_EXECUTABLE=str(BINARY))
    script = 'eval "$("$ACRE_EXECUTABLE" shell init bash)"\nprintf "PARENT=%s\\n" "$ACRE_SHELL_SESSION_ID"\n/bin/bash --noprofile --norc -c \'eval "$("$ACRE_EXECUTABLE" shell init bash)"; printf "CHILD=%s\\n" "$ACRE_SHELL_SESSION_ID"\'\n'
    p = f.cmd(['/bin/bash','--noprofile','--norc','-c',script], env=env)
    rows = dict(line.split('=',1) for line in p.stdout.splitlines() if '=' in line)
    assert rows['PARENT'] != rows['CHILD'] and rows['PARENT'] and rows['CHILD']
    return

def test_bundled_bash_completion_works():
    f = Fixture('bash-completion')
    env = dict(f.env, ACRE_EXECUTABLE=str(BINARY))
    script = 'eval "$("$ACRE_EXECUTABLE" shell init bash)"\nCOMP_WORDS=(acre ma)\nCOMP_CWORD=1\n_acre_completion\nprintf "%s\\n" "${COMPREPLY[@]}"\n'
    p = f.cmd(['/bin/bash','--noprofile','--norc','-c',script], env=env, check=False)
    assert p.returncode == 0 and 'main' in p.stdout.splitlines()
    return

def test_configured_default_branch_is_honored():
    f = Fixture('default-branch')
    f.git('config','init.defaultBranch','main')
    f.git('switch','-c','feature')
    f.write('feature-only','should not be a default base')
    f.commit()
    value = f.data('new','work','--stay')
    assert value['base_ref'] == 'main' and not (Path(value['path'])/'feature-only').exists()
    return

def test_fresh_fetches_explicit_nonpreferred_remote():
    f = Fixture('fresh-remote')
    old = f.git('rev-parse','HEAD').stdout.strip()
    f.git('remote','add','origin',str(f.repo))
    f.git('remote','add','upstream',str(f.repo))
    f.git('update-ref','refs/remotes/upstream/main',old)
    f.write('new-file','new commit')
    f.commit()
    new = f.git('rev-parse','HEAD').stdout.strip()
    result = f.data('new','work','--from','upstream/main','--fresh','--stay')
    actual = f.git('rev-parse','work').stdout.strip()
    assert actual == new and actual != old
    f.git('branch', 'not-fetched', new)
    result = f.data('new', 'unfetched', '--from', 'upstream/not-fetched', '--fresh', '--stay')
    assert result['ok'] and f.git('rev-parse', 'unfetched').stdout.strip() == new
    return

def test_json_commands_and_argument_errors_are_documents():
    f = Fixture('json-config')
    results = {}
    for args in [('config','set','pool.replenish','false'),('config','path'),('config','repo-init')]:
        p = f.acre(*args)
        assert json.loads(p.stdout)['ok'] is True
    p = f.acre('acquire','main',check=False)
    assert p.returncode == 2 and json.loads(p.stdout)['ok'] is False


def test_relative_roots_are_stable_after_navigation():
    f = Fixture('relative-root', {'root':'../acre-relative'})
    acquired = f.data('acquire','work','--new','--holder','audit')
    path = Path(acquired['path'])
    inspection = f.data('system','inspect',cwd=path)
    result = f.acre('release','--lease-id',acquired['lease_id'],cwd=path,check=False)
    assert len(inspection['state']['workspaces']) == 1 and result.returncode == 0
    return

def test_background_copy_revalidates_source():
    f = Fixture('source-copy-race')
    f.write('node_modules/version','old generation')
    bindir = f.root/'bin'
    bindir.mkdir()
    marker, gate = f.root/'paused', f.root/'continue'
    wrapper = bindir/'git'
    wrapper.write_text('#!/bin/sh\nfor arg do\n if [ "$arg" = check-ignore ]; then\n  pwd > "$AUDIT_MARKER"\n  while [ ! -f "$AUDIT_GATE" ]; do /bin/sleep 0.02; done\n fi\ndone\nexec /usr/bin/git "$@"\n')
    wrapper.chmod(0o755)
    env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'], AUDIT_MARKER=str(marker),AUDIT_GATE=str(gate))
    child = subprocess.Popen([str(BINARY),'__replenish',str(f.repo/'.git')],cwd=f.repo,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    try:
        deadline = time.monotonic()+10
        while not marker.exists() and time.monotonic()<deadline:
            time.sleep(0.01)
        assert marker.exists(), 'copy did not reach barrier'
        f.write('package.json','{"dependencies":{"changed":"2.0.0"}}\n')
        f.write('node_modules/version','new incompatible generation')
        gate.write_text('go')
        stdout,stderr = child.communicate(timeout=15)
        assert child.returncode == 0, (stdout,stderr)
    finally:
        gate.write_text('go')
        if child.poll() is None:
            child.kill()
            child.communicate()
    result = f.new()
    path = Path(result['path'])
    assert result['environment']['state'] == 'cold'
    assert (path/'package.json').read_text() == '{"name":"fixture"}\n'
    assert not (path/'node_modules/version').exists()
    return

def test_cache_seed_overlaps_are_rejected():
    f = Fixture('fork-seed-cache-overlap',{'environment':{'cacheRoots':['local'],'seedFiles':['local/.env']}})
    f.git('remote','add','origin','https://github.com/current/project.git')
    f.write('local/.env','SECRET=must-not-enter-fork\n')
    oid = f.git('rev-parse','HEAD').stdout.strip()
    bindir = f.root/'bin'
    bindir.mkdir()
    reply = {'number':8,'title':'Fork PR','author':{'login':'fork'},'url':'https://github.com/current/project/pull/8','baseRefName':'main','headRefName':'head','headRefOid':oid,'isCrossRepository':True,'headRepository':{'nameWithOwner':'fork/project'}}
    gh = bindir/'gh'
    gh.write_text('#!/usr/bin/python3\nprint('+repr(json.dumps(reply))+')\n')
    gh.chmod(0o755)
    env = dict(f.env,PATH=str(bindir)+':'+f.env['PATH'])
    result = f.data('pr:8',env=env,check=False)
    assert result['error']['code'] == 'ACRE_CONFIG_INVALID'
    assert f.data('system','inspect')['state']['workspaces'] == []


def test_repair_preserves_external_registrations():
    f = Fixture('external-repair')
    external = f.root/'external'
    f.git('worktree','add','-b','external',str(external))
    external.rename(f.root/'temporarily-away')
    before = f.git('worktree','list','--porcelain').stdout
    result = f.data('system','repair')
    after = f.git('worktree','list','--porcelain').stdout
    assert str(external) in before and str(external) in after
    return

def test_process_probe_failure_prevents_reuse():
    f = Fixture('process-scanner-failure')
    path = Path(f.new()['path'])
    f.data('done','work')
    child = subprocess.Popen(['/bin/sleep','30'],cwd=path)
    try:
        bindir = f.root/'bin'
        bindir.mkdir()
        lsof = bindir/'lsof'
        lsof.write_text('#!/bin/sh\nexit 2\n')
        lsof.chmod(0o755)
        env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'])
        result = f.new('next',env=env)
        assert Path(result['path']) != path and child.poll() is None
        done = f.data('done', 'next', env=env, check=False)
        assert done['retained'] and done['assessment']['processCheckError']
        acquired = f.data('acquire', 'leased', '--new', '--holder', 'test', env=env)
        released = f.data('release', '--lease-id', acquired['lease_id'], env=env)
        assert released['ok'] and released['retained']
        assert Path(acquired['path']).exists()
        return
    finally:
        child.terminate()
        child.wait()

def test_index_failure_does_not_issue_a_lease():
    f = Fixture('missing-index-write')
    root = Path(f.settings['root'])
    root.mkdir()
    (root/'repositories.json').mkdir()
    acquired = f.data('acquire','work','--new','--holder','audit',check=False)
    assert acquired['ok'] is False
    (root/'repositories.json').rmdir()
    assert f.data('system','inspect')['state']['leases'] == []


def test_head_suffixed_branches_are_selectable():
    f = Fixture('branch-head-suffix')
    f.git('branch','feature/HEAD')
    p = f.acre('feature/HEAD',check=False)
    assert p.returncode == 0 and json.loads(p.stdout)['ok']
    return

def test_newline_repository_paths_are_supported():
    f = Fixture('newline-path')
    moved = f.root/'repo\nwith-newline'
    f.repo.rename(moved)
    f.repo = moved
    p = f.acre('system','inspect',check=False)
    assert p.returncode == 0 and json.loads(p.stdout)['ok']
    return

def test_done_preserves_bisect():
    f = Fixture('bisect')
    old = f.git('rev-parse','HEAD').stdout.strip()
    f.write('commit2','second')
    f.commit()
    f.write('commit3','third')
    f.commit()
    path = Path(f.new()['path'])
    f.git('bisect','start','--no-checkout','HEAD',old,cwd=path)
    bisect_log = Path(f.git('rev-parse','--git-path','BISECT_LOG',cwd=path).stdout.strip())
    assert bisect_log.exists()
    f.data('system','warm')
    p = f.acre('done','work',check=False)
    value = json.loads(p.stdout)
    assert p.returncode == 6 and path.exists() and bisect_log.exists()
    assert value['assessment']['operation'] == 'bisect'
    return

def test_plain_output_is_ascii():
    f = Fixture('plain-output')
    p = f.cmd([BINARY,'--plain','new','work','--stay'])
    nonascii = sorted(set(char for char in p.stdout if ord(char)>127))
    assert not nonascii
    return

def test_child_interrupt_preserves_exit_status():
    f = Fixture('child-interrupt')
    p = f.cmd([BINARY,'main','--','/bin/sh','-c','kill -INT $$'],check=False)
    assert p.returncode == 130
    return

def test_setup_preserves_unreadable_startup_files():
    f = Fixture('setup-non-utf8')
    fakehome = f.root/'home'
    fakehome.mkdir()
    rc = fakehome/'.bashrc'
    original = b'export KEEP_ME=yes\n# non-UTF8 comment: \xff\n'
    rc.write_bytes(original)
    env = dict(f.env,HOME=str(fakehome))
    result = f.data('setup','--shell','bash','--yes',env=env,check=False)
    assert result['ok'] is False and rc.read_bytes() == original


def test_cross_repository_navigation_moves_shell_lease():
    f = Fixture('cross-repo-leases')
    other = Fixture('cross-repo-other')
    p,fields,_ = f.shell('new','first')
    assert p.returncode == 0
    first = Path(fields[2].decode())
    p,fields,_ = f.shell('new','second',cwd=other.repo)
    assert p.returncode == 0
    first_leases = f.data('system','inspect')['state']['leases']
    second_leases = f.data('system','inspect',cwd=other.repo)['state']['leases']
    assert first_leases == [] and len(second_leases)==1
    return

def test_setup_targets_active_shell_profiles():
    evidence = {}
    for kind in ['bash','zsh']:
        f = Fixture('setup-profile-'+kind)
        fakehome = f.root/'home'
        fakehome.mkdir()
        bindir = f.root/'bin'
        bindir.mkdir()
        (bindir/'acre').symlink_to(BINARY)
        env = dict(f.env,HOME=str(fakehome),PATH=str(bindir)+':'+f.env['PATH'])
        if kind == 'zsh':
            zdotdir = fakehome/'zsh-config'
            zdotdir.mkdir()
            env['ZDOTDIR'] = str(zdotdir)
        setup = f.data('setup','--shell',kind,'--yes',env=env)
        if kind == 'bash':
            p = f.cmd(['/bin/bash','--login','-c','type -t acre'],env=env)
            assert p.stdout.strip() == 'function',p.stdout
        else:
            p = f.cmd(['/bin/zsh','-i','-c','whence -w acre'],env=env)
            assert p.stdout.strip() == 'acre: function',p.stdout
        evidence[kind] = {'setup_path':setup['shell_file'],'resolved_acre':p.stdout.strip(),'function_loaded':True}
    return

def test_pull_in_branch_names_is_not_a_pr():
    f = Fixture('pull-branch-selector')
    f.git('branch','feature/pull/7')
    p = f.acre('feature/pull/7',check=False)
    data = json.loads(p.stdout)
    assert data['ok'] is True
    return

def test_done_resume_updates_destination_lease_and_history():
    f = Fixture('done-navigation')
    p,fields,_ = f.shell('new','first')
    assert p.returncode == 0
    first = Path(fields[2].decode())
    p,fields,_ = f.shell('new','second',cwd=first)
    assert p.returncode == 0
    second = Path(fields[2].decode())
    p,fields,_ = f.shell('done',cwd=second)
    assert p.returncode == 194 and Path(fields[2].decode()) == first
    p,_,_ = f.shell('__resume',fields[3].decode(),cwd=first)
    assert p.returncode == 0
    state = f.data('system','inspect')['state']
    assert len(state['leases']) == 1
    first_id = next(w['id'] for w in state['workspaces'] if Path(w['path']) == first)
    assert state['leases'][0]['workspaceId'] == first_id
    history = json.loads((Path(f.settings['root'])/'shells/audit-session.json').read_text())
    # The returned workspace is an idle slot now; history must not lead the shell back into it.
    assert Path(history['currentDirectory']) == first and Path(history['previousDirectory']) != second
    p,fields,_ = f.shell('-',cwd=first)
    assert len(fields) < 3 or Path(fields[2].decode()) != second
    return

def test_standalone_completion_works():
    f = Fixture('standalone-completion')
    script = f.cmd([BINARY,'completion','zsh']).stdout
    path = f.root/'completion.zsh'
    path.write_text(script)
    bindir = f.root/'bin'
    bindir.mkdir()
    (bindir/'acre').symlink_to(BINARY)
    env = dict(f.env,PATH=str(bindir)+':'+f.env['PATH'])
    # Stub only Zsh's renderer so completion values can be observed outside an interactive prompt.
    command = 'compdef() { :; }; compadd() { print -l -- "${(@P)2}"; }; source "$1"; words=(acre ma); CURRENT=2; _acre_completion'
    p = f.cmd(['/bin/zsh','-f','-c',command,'zsh',path],env=env)
    assert 'main' in p.stdout.splitlines()
    direct = f.cmd([BINARY,'__complete','ma'])
    assert 'main' in direct.stdout
    return

def test_directory_override_applies_to_worktree_selectors():
    f = Fixture('directory-selector')
    normal = f.data('worktree:.')
    p = f.acre('-C',str(f.repo),'worktree:.',cwd=f.root,check=False)
    assert normal['ok'] and p.returncode == 0 and json.loads(p.stdout)['path'] == normal['path']
    return

def test_enterprise_requests_keep_their_host():
    results = {}
    for host in ['github.corp.example','git.corp.example']:
        f = Fixture('enterprise-'+host.split('.')[0])
        f.git('remote','add','origin','https://'+host+'/current/project.git')
        bindir = f.root/'bin'
        bindir.mkdir()
        oid = f.git('rev-parse','HEAD').stdout.strip()
        reply = {'number':9,'title':'Enterprise PR','author':{'login':'audit'},'url':'https://'+host+'/current/project/pull/9','baseRefName':'main','headRefName':'head','headRefOid':oid,'isCrossRepository':False}
        gh = bindir/'gh'
        gh.write_text('#!/usr/bin/python3\nimport sys,json\nfrom pathlib import Path\nPath('+repr(str(f.root/'gh-args.json'))+').write_text(json.dumps(sys.argv[1:]))\nprint('+repr(json.dumps(reply))+')\n')
        gh.chmod(0o755)
        env = dict(f.env,PATH=str(bindir)+':'+f.env['PATH'])
        p = f.acre('pr:9',env=env,check=False)
        if host.startswith('github'):
            args = json.loads((f.root/'gh-args.json').read_text())
            assert args[args.index('--repo')+1]==host+'/current/project'
            results[host] = {'repo_argument':args[args.index('--repo')+1]}
            seen = []
            class Proxy(http.server.BaseHTTPRequestHandler):
                def do_CONNECT(self):
                    seen.append(self.path)
                    self.send_error(502, 'Audit proxy: no upstream connection made')
                def log_message(self, *args):
                    pass
            server = http.server.HTTPServer(('127.0.0.1',0), Proxy)
            thread = threading.Thread(target=server.serve_forever,daemon=True)
            thread.start()
            try:
                realenv = {key:value for key,value in f.env.items() if not key.startswith(('GH_','GITHUB_')) and key.lower() not in ['http_proxy','https_proxy','all_proxy','no_proxy']}
                realenv.update(GH_TOKEN='audit-placeholder-token', GH_ENTERPRISE_TOKEN='audit-placeholder-token', GH_CONFIG_DIR=str(f.root/'gh-config'), HTTPS_PROXY='http://127.0.0.1:'+str(server.server_port), NO_PROXY='')
                f.acre('pr:10',env=realenv,check=False)
                assert seen and all(value==host+':443' for value in seen),seen
                results[host]['actual_gh_connection_targets'] = seen
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
        else:
            data = json.loads(p.stdout)
            assert data['ok'] is True
            args = json.loads((f.root/'gh-args.json').read_text())
            assert args[args.index('--repo')+1] == host+'/current/project'
            results[host]=data
    return

def test_new_copies_report_actual_strategy():
    f = Fixture('clone-mode')
    f.write('node_modules/marker','copied from primary')
    assert f.data('system','inspect')['state']['slots']==[]
    result = f.new()
    assert (Path(result['path'])/'node_modules/marker').read_text()=='copied from primary'
    environment = result['environment']
    assert not result['reused'] and environment['cloneMode'] in ['copy','reflink']
    # A copy must be counted; a reflink claim with nothing counted is only honest for a real clone.
    if environment['cloneMode'] == 'copy':
        assert environment['clonedFiles'] == 1 and environment['clonedBytes'] == len('copied from primary')
    if sys.platform == 'darwin':
        apfs = subprocess.run(['/bin/df','-T','apfs',str(f.root)],capture_output=True).returncode == 0
        assert environment['cloneMode'] == ('reflink' if apfs else 'copy')
    assert Path(result['environment']['source']) == f.repo
    return

def test_pid_probe_failure_preserves_live_leases():
    f = Fixture('pid-probe-failure')
    acquired = f.data('acquire','work','--new','--holder','audit','--pid',str(os.getpid()))
    bindir = f.root/'bin'
    bindir.mkdir()
    ps = bindir/'ps'
    ps.write_text('#!/bin/sh\nexit 2\n')
    ps.chmod(0o755)
    env = dict(f.env,PATH=str(bindir)+':'+f.env['PATH'])
    done = f.data('done','work',env=env,check=False)
    assert not done['ok'] and len(f.data('system','inspect')['state']['leases']) == 1
    return

def test_nested_cache_paths_respect_seed_boundaries():
    f = Fixture('nested-cache-seed', {'environment': {'seedFiles': ['local']}})
    f.git('remote', 'add', 'origin', 'https://github.com/current/project.git')
    f.write('local/node_modules/secret', 'synthetic directory seed')
    oid = f.git('rev-parse', 'HEAD').stdout.strip()
    bindir = f.root/'bin'
    bindir.mkdir()
    reply = {'number': 11, 'title': 'Fork', 'url': 'https://github.com/current/project/pull/11',
             'baseRefName': 'main', 'headRefName': 'head', 'headRefOid': oid,
             'isCrossRepository': True}
    gh = bindir/'gh'
    gh.write_text('#!/usr/bin/python3\nprint('+repr(json.dumps(reply))+')\n')
    gh.chmod(0o755)
    env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'])
    result = f.data('pr:11', env=env)
    assert not (Path(result['path'])/'local/node_modules/secret').exists()
    warmed = f.data('system', 'warm')
    slot = Path(warmed['slots'][0]['path'])
    f.write('local/node_modules/secret', 'legacy cached seed', root=slot)
    reply['number'] = 12
    reply['url'] = 'https://github.com/current/project/pull/12'
    gh.write_text('#!/usr/bin/python3\nprint('+repr(json.dumps(reply))+')\n')
    result = f.data('pr:12', env=env)
    assert Path(result['path']) != slot
    assert not (Path(result['path'])/'local/node_modules/secret').exists()
    assert (slot/'local/node_modules/secret').read_text() == 'legacy cached seed'

def test_seed_paths_are_normalized_before_trust_checks():
    for index, seed in enumerate(['.//local', 'local/./', './local//']):
        f = Fixture('normalized-seed-' + str(index), {'environment': {'seedFiles': [seed]}})
        f.write('local/node_modules/secret', 'synthetic seed data')
        f.git('remote', 'add', 'origin', 'https://github.com/current/project.git')
        oid = f.git('rev-parse', 'HEAD').stdout.strip()
        bindir = f.root/'bin'
        bindir.mkdir()
        reply = {'number': 17, 'title': 'Fork', 'url': 'https://github.com/current/project/pull/17',
                 'baseRefName': 'main', 'headRefName': 'head', 'headRefOid': oid,
                 'isCrossRepository': True}
        gh = bindir/'gh'
        gh.write_text('#!/usr/bin/python3\nprint(' + repr(json.dumps(reply)) + ')\n')
        gh.chmod(0o755)
        env = dict(f.env, PATH=str(bindir)+':'+f.env['PATH'])
        result = f.data('pr:17', env=env)
        assert not (Path(result['path'])/'local/node_modules/secret').exists(), seed
        trusted = Path(f.new('trusted')['path'])
        assert (trusted/'local/node_modules/secret').read_text() == 'synthetic seed data', seed
        assert f.data('done', 'trusted')['ok']
        assert not (trusted/'local').exists(), seed


def test_process_detection_handles_escaped_directory_names():
    for index, name in enumerate(['line\nbreak', 'literal\\n', 'control\x01char', 'caret^A', 'café']):
        f = Fixture('process-escape-' + str(index) + '-' + name)
        path = Path(f.new()['path'])
        f.data('system', 'warm', '--slots', '1')
        child = subprocess.Popen(['/bin/sleep', '30'], cwd=path)
        try:
            result = f.data('done', 'work', check=False)
            assert not result['ok'] and result['retained'] and path.exists(), result
            assert any(process['pid'] == child.pid for process in result['assessment']['processes']), result
            assert child.poll() is None
        finally:
            child.terminate()
            child.wait()


TESTS = [value for name, value in list(globals().items())
         if name.startswith('test_') and callable(value)]
if os.environ.get('AUDIT_FILTER'):
    TESTS = [test for test in TESTS if test.__name__ in os.environ['AUDIT_FILTER'].split(',')]
if not TESTS:
    raise SystemExit('No matching regression tests')
results = []
for test in TESTS:
    try:
        test()
        item = {'test': test.__name__, 'passed': True}
    except Exception:
        item = {'test': test.__name__, 'passed': False, 'error': traceback.format_exc()}
    results.append(item)
    print(json.dumps(item), flush=True)
(RUN/'results.json').write_text(json.dumps(results, indent=2))
print('EVIDENCE_DIRECTORY='+str(RUN), flush=True)
sys.exit(0 if all(result['passed'] for result in results) else 1)
