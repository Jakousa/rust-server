import os
def fix_file(path):
    with open(path, 'r') as f:
        lines = f.readlines()

    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        out.append(line)
        if 'uses: arduino/setup-protoc@v3' in line:
            if i+2 < len(lines) and 'with:' in lines[i+1] and 'repo-token:' in lines[i+2]:
                out.append(lines[i+1])
                out.append(lines[i+2].replace('\n', '') + '\n')
                out.append('          version: "25.1"\n')
                i += 2
        i += 1

    with open(path, 'w') as f:
        f.writelines(out)

fix_file('.github/workflows/rust.yml')
fix_file('.github/workflows/mutation-tests.yml')
