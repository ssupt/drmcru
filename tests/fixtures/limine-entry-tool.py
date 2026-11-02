#!/usr/bin/env python3
"""Temporary-root test double for the supported Limine command interface."""
import glob
import os
from pathlib import Path
import re
import shutil
import sys

root = Path(os.environ['TEST_ROOT'])
machine = '0' * 32
args = sys.argv[1:]

def resolve(key):
    values = {}
    paths = [root / 'entry-base.conf', *map(Path, sorted(glob.glob(str(root / 'entry-tool/*.conf')))), root / 'default.conf']
    for path in paths:
        if not path.is_file():
            continue
        for line in path.read_text().splitlines():
            match = re.match(r'^\s*KERNEL_CMDLINE\[([^]]+)\]\s*(\+?=)\s*(.*)$', line)
            if match:
                name, operator, value = match.groups()
                if value.startswith('"') and value.endswith('"'):
                    value = value[1:-1]
                values[name] = (values.get(name, '') + ' ' + value) if operator == '+=' else value
    if key in values:
        return values[key].strip()
    if 'fallback' in key and 'fallback' in values:
        return values['fallback'].strip()
    return values.get('default', (root / 'kernel-cmdline').read_text()).strip()

if args[0] == '--get-cmdline':
    if (root / 'fail-query').exists():
        raise SystemExit(42)
    print(resolve(args[1]))
    sys.exit(0)

boot = root / 'boot'
if args[0] in ('--remove-kernel', '--remove-uki'):
    name = args[1]
    configuration = boot / 'limine.conf'
    records = re.split(r'(?=^/)', configuration.read_text(), flags=re.M)
    kept = []
    for record in records:
        heading = record.splitlines()[0].lstrip('/+').strip() if record else ''
        if args[0] == '--remove-kernel':
            discard = heading in (name, name + '-fallback') and 'protocol: linux' in record
        else:
            discard = heading == name and 'protocol: efi' in record
        if not discard:
            kept.append(record)
    configuration.write_text(''.join(kept))
    if args[0] == '--remove-kernel':
        shutil.rmtree(boot / machine / name)
    else:
        (boot / 'EFI/Linux' / f'{machine}_{name}.efi').unlink()
    sys.exit(0)
if args[0] == '--add-kernel':
    name, kernel, image = args[1:4]
    fallback = len(args) > 4 and args[4] == '-fallback'
    key = name + ('-fallback' if fallback else '')
    directory = boot / machine / name
    directory.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(kernel, directory / 'vmlinuz')
    target = directory / Path(image).name
    shutil.copyfile(image, target)
    lines = [f'/{key}', '  protocol: linux', f'  path: boot():/{machine}/{name}/vmlinuz',
             f'  module_path: boot():/{target.relative_to(boot)}', f'  cmdline: {resolve(key)}']
elif args[0] == '--add-uki':
    key, image = args[1:3]
    target = boot / 'EFI/Linux' / f'{machine}_{key}.efi'
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(image, target)
    lines = [f'/{key}', '  protocol: efi', f'  path: boot():/{target.relative_to(boot)}', f'  cmdline: {resolve(key)}']
else:
    raise SystemExit(f'Unexpected Limine command: {args}')

configuration = boot / 'limine.conf'
kept = []
discard = False
for line in configuration.read_text().splitlines():
    if line.startswith('/'):
        discard = line.lstrip('/+').strip() == key
    if not discard:
        kept.append(line)
configuration.write_text('\n'.join(kept + lines) + '\n')
counter = root / 'install-count'
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
if (root / 'fail-second-install').exists() and count == 2:
    raise SystemExit(42)
if (root / 'fail-after-install').exists():
    raise SystemExit(42)
