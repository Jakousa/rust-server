sed -i 's/repo-token: \${{ secrets.GITHUB_TOKEN }}/repo-token: \${{ secrets.GITHUB_TOKEN }}\n          version: "25.1"/g' .github/workflows/rust.yml
sed -i 's/repo-token: \${{ secrets.GITHUB_TOKEN }}/repo-token: \${{ secrets.GITHUB_TOKEN }}\n          version: "25.1"/g' .github/workflows/mutation-tests.yml
