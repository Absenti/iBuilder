# Engine patches kept for a decision (never applied)

`scripts/build-host.sh` applies `engine-patches/0[45]*.patch` only; nothing in this folder. Why each is here: `docs/ENGINE.md`.

* `0450-nvn-suspend-bound-graphics-wait.patch` (nxnative 6df16dbe): breaks the archive write before a kill after backgrounding (`t_pso_binary_archive`).
* `0452-nvn-reject-missing-pipeline-functions.patch` (nxnative d0824312): 9 checks of `t_pso_pending` fail with it on this tree.
* `0459-game-ocarina-alias-8gib-DECISION.patch` (nxnative f7ae8a3a): Ocarina's 8 GiB alias (16.5 GiB reservation): owner decision, unverified on an iPhone 15 Pro.
