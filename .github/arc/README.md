# k0s ARC

The `homelab2` k0s controller is `192.168.2.10` (SSH port `2222`). ARC is
pinned to chart `0.14.2`; runner credentials stay in the external
`arc-runners/arc-github-auth` Secret under the `github_token` key.

Build the single-node runner image and import its fixed tag into k0s/containerd:

```bash
docker buildx build --platform linux/amd64 --load \
  -f .github/arc/Dockerfile \
  -t localhost/den-arc-runner:2.337.0-r1 .
docker save localhost/den-arc-runner:2.337.0-r1 | \
  ssh -p 2222 192.168.2.10 'sudo k0s ctr images import -'
```

Install or update from the repository root. These commands deliberately use
the trusted SSH path rather than whichever local kube context happens to be
current:

```bash
ssh -p 2222 192.168.2.10 \
  'sudo helm --kubeconfig /var/lib/k0s/pki/admin.conf upgrade --install arc \
    oci://ghcr.io/actions/actions-runner-controller-charts/gha-runner-scale-set-controller \
    --version 0.14.2 -n arc-systems --create-namespace -f -' \
  < .github/arc/controller-values.yaml

ssh -p 2222 192.168.2.10 \
  'sudo k0s kubectl create namespace arc-runners --dry-run=client -o yaml \
    | sudo k0s kubectl apply -f -'
ssh -p 2222 192.168.2.10 'sudo k0s kubectl apply -f -' \
  < .github/arc/cluster.yaml

for mode in runners dind kubernetes; do
  ssh -p 2222 192.168.2.10 \
    "sudo helm --kubeconfig /var/lib/k0s/pki/admin.conf upgrade --install k0s-arc-$mode \
      oci://ghcr.io/actions/actions-runner-controller-charts/gha-runner-scale-set \
      --version 0.14.2 -n arc-runners -f -" \
    < ".github/arc/$mode-values.yaml"
done
ssh -p 2222 192.168.2.10 'sudo k0s kubectl apply -f -' \
  < .github/arc/heartbeat.yaml
```

Changing `githubConfigUrl` changes scale-set identity. Uninstall and reinstall
that scale-set release instead of upgrading the URL in place. Roll back normal
changes with `helm rollback RELEASE REVISION -n NAMESPACE`.
