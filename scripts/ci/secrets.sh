#!/usr/bin/env bash
set -euo pipefail
# Scan the complete current source tree, including new files but excluding build
# output, ignored local credentials, symlink targets, and Git's object database.
scan_directory="$(mktemp -d)"
trap 'rm -rf "$scan_directory"' EXIT
python3 - "$scan_directory" <<'PY'
import pathlib, shutil, subprocess, sys
root = pathlib.Path.cwd()
destination = pathlib.Path(sys.argv[1])
files = subprocess.check_output([
    'git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'
]).split(b'\0')
for raw in files:
    if not raw:
        continue
    path = pathlib.Path(raw.decode())
    source = root / path
    if source.is_file() and not source.is_symlink():
        target = destination / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
PY
report_path="${GITLEAKS_REPORT:-dependency-evidence/secrets.json}"
mkdir -p "$(dirname "$report_path")"
scan_status=0
gitleaks dir "$scan_directory" --config "$PWD/.gitleaks.toml" --redact --no-banner --exit-code 1 \
  --report-format json --report-path "$report_path" || scan_status=$?
python3 - "$scan_directory" "$report_path" <<'PY'
import json, pathlib, sys
prefix = sys.argv[1] + '/'
report = pathlib.Path(sys.argv[2])
if report.exists():
    findings = json.loads(report.read_text())
    for finding in findings:
        for field in ('File', 'Fingerprint'):
            if field in finding:
                finding[field] = finding[field].replace(prefix, '')
    report.write_text(json.dumps(findings, indent=2) + '\n')
PY
exit "$scan_status"
