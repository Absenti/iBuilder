#!/bin/zsh
# private workspace (the shared one does not load while another contributor's crate is half written): sync this crate, run cargo there (under $IBUILDER_SCRATCH_OLD, else $IBUILDER_SCRATCH)
R=$(cd "$(dirname "$0")/../../../.." && pwd)   # the repository
[ -f "$R/scripts/local.env" ] && . "$R/scripts/local.env"   # scripts/local.env.example
S=${IBUILDER_SCRATCH_OLD:-${IBUILDER_SCRATCH:?IBUILDER_SCRATCH is not set (the scratch folder; environment or scripts/local.env, see scripts/local.env.example)}}
mkdir -p $S/ws/crates
rsync -a --delete --exclude target $R/crates/ibuilder-shader/ $S/ws/crates/ibuilder-shader/
rsync -a --delete --exclude target $R/crates/ibuilder-testkit/ $S/ws/crates/ibuilder-testkit/
sed 's#members = \["crates/\*"\]#members = ["crates/ibuilder-shader", "crates/ibuilder-testkit"]#' $R/Cargo.toml > $S/ws/Cargo.toml
sed -i '' -e '/^thiserror/d' -e '/^anyhow/d' $S/ws/Cargo.toml
rm -f $S/ws/Cargo.lock
cd $S/ws && CARGO_TARGET_DIR=$S/target-shader cargo "$@"
