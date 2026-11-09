def process(fname):
    with open(fname) as f:
        content = f.read()
    content = content.replace("uses: arduino/setup-protoc@v3\n        with:\n          repo-token: ${{ secrets.GITHUB_TOKEN }}", "uses: arduino/setup-protoc@v3\n        with:\n          version: '25.x'\n          repo-token: ${{ secrets.GITHUB_TOKEN }}")
    with open(fname, 'w') as f:
        f.write(content)

process('.github/workflows/rust.yml')
process('.github/workflows/mutation-tests.yml')
