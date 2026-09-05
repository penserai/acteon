import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const directory = mkdtempSync(join(tmpdir(), 'acteon-package-'));
try {
  const [packed] = JSON.parse(execFileSync('npm', ['pack', '--json', '--pack-destination', directory], { encoding: 'utf8' }));
  if (packed.files.some(({ path }) => path.includes('.test.'))) throw new Error('test files leaked into package');
  writeFileSync(join(directory, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  execFileSync('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', join(directory, packed.filename)], { cwd: directory, stdio: 'pipe' });
  execFileSync(process.execPath, ['--input-type=module', '-e', "import { ActeonClient } from '@acteon/client'; if (typeof ActeonClient !== 'function') throw new Error('missing export');"], { cwd: directory, stdio: 'inherit' });
  console.log('Packed client installs and imports successfully.');
} finally {
  rmSync(directory, { recursive: true, force: true });
}
