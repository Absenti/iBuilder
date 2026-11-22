# South Park: The Fractured But Whole 1.05: platform-limit cuts for Snowdrop's three-pool page allocator, the same routine as Mario + Rabbids Kingdom
# Battle's (010067300059A000) built from an older source. Its initializer (+0x17fb29c onwards) reserves one 12 GB address range (+0x17fb2b0,
# AllocateAddressRegion through +0x185ff04) and splits it at fixed offsets: large allocations [base, base+4 GB) with a 65,536-page limit (w5 at
# +0x17fb2dc for the large allocator's init +0x17fef14), 512 KB pages [align(base+4 GB, 512 KB), base+8 GB) (+0x17fb300..+0x17fb30c) and 64 KB pages
# [align(base+8 GB, 64 KB), base+12 GB) (+0x17fb388 and x22 from +0x17fb2b8). 12 GB cannot fit the 4.375 GB alias region an iPhone has (the NSP
# script's region check measured alias +13.16 GB in nnMain's first 15 s).
# These cuts keep the layout and shrink it to 4 GB, sized from the original layout's use on the Mac (64 GB alias region, the pools' bump pointers and
# the large allocator's page counts read with DM_CTL memdump, 2026-10-31): after 90 s (title screen, then the opening logos) 64 KB pages had reached
# 848 MB (735 MB at 30 s), 512 KB pages 236 MB, large allocations 16,384 pages (1 GB). So: 1.5 GB large (24,576 pages), 512 MB of 512 KB pages,
# 2 GB of 64 KB pages. A first cut of 2 GB / 512 MB / 512 MB ran the 64 KB pool out in 2 s: its bump path (+0x17fb600) then asks for another 4 GB
# at the range's end (+0x17fb640), a fixed-address reservation the SDK aborts on (Break at sdk+0xa9dec, through +0x8ce0c). With 4 GB, the other
# regions (64 MB node area, about 2 MB of small ones before it, 104 MB after it) leave about 146 MB of the alias region.
# free tells the classes apart by the stored range starts (+0xdf0c8, +0xdf0f8; +0x17fce98, +0x17fceac), never by fixed offsets, and the slab paths
# take their range from the same fields (+0x17fc3e8, +0x17fc830). The SDK's 1 GB node area and 1 GB region alignment are cut in
# dm/tools/sdk_patches.py. The initializer and its direct callees match uniquely in 1.0 and 1.05. Pool use over sustained play is open (README.md, Memory).
MATCH_CUTS = {
    'main': [{
        'name': 'Snowdrop three-pool allocator: 12 GB of ranges → 4 GB',
        'size': 1068,
        'fingerprints': ['eb0c76d0957ce3eb60b3cb14ff72cf2bd128295bd266981699c20d0cccb43b80'],
        'cuts': [
            (0x2a8, 'e00760b2', 'e00360b2', 'Snowdrop allocator: reserve 4 GB instead of 12 GB'),
            (0x2b0, 'f60760b2', 'f60360b2', 'Snowdrop allocator: combined range end 12 GB → 4 GB'),
            (0x2d4, 'e5031032', '05008c52', 'Snowdrop large allocator: maximum 64 KB page count 65536 → 24576, matching its 1.5 GB range'),
            (0x2f8, '2a00c0d2', 'ea00acd2', 'Snowdrop 512 KB pool start: load the high part of 1.5 GB + 0x7ffff'),
            (0x2fc, 'ea00a0f2', 'eaff9ff2', 'Snowdrop 512 KB pool start: repeat the next movk as the high part now fits one instruction'),
            (0x304, 'eb035fb2', '0b00b0d2', 'Snowdrop 512 KB pool end: base + 8 GB → base + 2 GB'),
            (0x380, 'ea031fb2', '0a00b0d2', 'Snowdrop 64 KB pool start: load the high part of 2 GB + 0xffff'),
        ],
    }],
}

# How general each patch is and could be (dm/tools/docs_patches.py "Binding", listed in docs/agents/game-patches.md): what, its binding now, the loosest it could have (goal),
# purpose, why; blocked or kept: why the gap stays open (cannot close yet, or left open on purpose); checked: what was checked on game files (else a judgement)
BINDING = [
    {'what': 'Snowdrop three-pool allocator: 12 GB of ranges → 4 GB', 'now': 'game', 'goal': 'engine', 'purpose': 'address space',
     'checked': 'unique function and original bytes on 1.0 and 1.05; pool use measured on the Mac, 2026-10-31',
     'why': "the older initializer holds the pool bounds in separate immediates; its complete function moves between game builds. The 4 GB size works around the iPhone's address-space limit",
     'blocked': 'only this game has been checked with the older initializer and this allocation split'},
]

# iBuilderRust overlay (crates/ibuilder-rewrite/tools/overlay): the SDK 3.5.1 half of nxnative e74c3fa7's dm/tools/sdk_patches.py, the older VammManager's 1 GB node area
# and 1 GB region alignment, which this game's sdk (8bb65519) needs so Snowdrop's 4 GB region fits the iPhone's alias region after its metadata. Upstream makes it an SDK-wide rule
# (every game's sdk of a build with no CUTS); here it is this profile's rule for its 'sdk' module, so no other game's rewrite (nor its match.tsv) changes. Kingdom Battle's SDK 4.4.0
# keeps its registered CUTS (9de92bad), the same three cuts.
def _old_vamm(node_size, occupied, node_fingerprint, align_fingerprint):
    """Older VammManager implementations reserve a full 1 GB of nodes. Cut both functions together so Snowdrop's region fits on iPhone after its metadata."""
    return {'name': 'Older VammManager: 64 MB node area and 32 MB large-region alignment', 'functions': [
        {'size': node_size, 'fingerprints': [node_fingerprint], 'cuts': [
            (0x24, 'e9030232', '0980a052', 'VammManager node area limit: 1 GB to 64 MB'),
            (occupied, 'ea030232', '0a80a052', 'VammManager occupied node area: 1 GB to 64 MB'),
        ]},
        {'size': 328, 'fingerprints': [align_fingerprint], 'cuts': [
            (0x60, 'ea030232', '0a40a052', 'AllocateAddressRegion: align regions of 1 GB and more to 32 MB'),
        ]},
    ]}


# SDK 3.5.1 (South Park 1.0 / 1.05). Reproduces all three original cut sites and replacement bytes (upstream's note).
MATCH_CUTS['sdk'] = [_old_vamm(288, 0xb8, '1afa13cc6a6b823fde0b57f324da45b2dfbebfacb0d8dd09158a150d3d006609', '3726c7d94ae67a37e2c30df6ccf76e8ca7e9204a83b05cedb7a7631e8b3b5bab')]
