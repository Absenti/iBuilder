#!/usr/bin/env python3
"""make-free-fonts.py: the six "shared font" stand-ins for the app bundle (dm/font/), made ONLY from a freely licensed font, so they can be redistributed in a public IPA.

    make-free-fonts.py <engine_tools_dir> <source_font> <out_dir> [--max-err 1.0] [--license ofl|apache] [--keep-converted FILE]

  <engine_tools_dir>  the engine's dm/tools folder (read only): make_shared_fonts.py and make_ext_font.py are LOADED from there (importlib), never copied, so the cut, the character
                      sets per type (standard / Simplified / Simplified ext / Traditional / Korean), the names ("DM Standard", ...) and the drawing of the button icons stay the engine's.
  <source_font>       a freely licensed font: Noto Sans CJK JP Regular (NotoSansCJKjp-Regular.otf, SIL OFL 1.1, the font this script was written for), or a TrueType font
                      (DroidSansFallbackFull.ttf, Apache-2.0, was measured too: it has no modern Hangul, so it cannot make korean.ttf). Its licence is read from its name table (IDs 13/14); with none of OFL or Apache
                      named the script stops (--license says it by hand). A font under /System/Library is refused, whatever its licence says.
  <out_dir>           receives standard.ttf, chinese_simplified.ttf, chinese_simplified_ext.ttf, chinese_traditional.ttf, korean.ttf, nintendo_ext.ttf and LICENSE-FONTS.txt (the font's
                      copyright line, its licence, what was changed). Nothing is written in <engine_tools_dir>, and nothing is written to <out_dir> unless every check passed.
  --max-err           CFF to TrueType: the largest distance, in font units of a 1000-unit em, between a cubic curve and its quadratic replacement (default 1.0, 0.1 % of the em).
  --keep-converted    also save the converted (TrueType) source font there, for inspection.

No network, no key, no system font. Needs fontTools (pip install fonttools), as the engine's scripts do.

How the CFF outlines of Noto become TrueType outlines (the game's font engine loads Nintendo's shared fonts, which are TrueType, glyf-based; its note says what it needs grows with the
font's hinting program and outlines): (1) the OTF is cut down to the union of the five character sets, hinting dropped (hint operators of the charstrings, GSUB/GPOS/GDEF/BASE/VORG/vhea/vmtx/DSIG):
65,535 glyphs become about 34,000; (2) every glyph is drawn through fontTools' Cu2QuPen (cubic to quadratic Bezier, error at most --max-err, reverse_direction=True: CFF outers run
counter-clockwise, TrueType's run clockwise) into a TTGlyphPen (coordinates rounded to integers, on-curve points implied by their neighbours dropped: the same shape, fewer points); (3) the CFF table is
replaced by glyf and loca, hmtx gets the new left side bearings, maxp becomes version 1.0 with the real maxima, post format 3 (no glyph names), the sfnt version 0x00010000. The engine's own cut then
runs on that TrueType font exactly as it does on Arial Unicode. The result is deterministic: the timestamps are fixed to the source font's (SOURCE_DATE_EPOCH) and every step is ordered.

Names: the engine's cut renames the family, full, unique and PostScript names (IDs 1, 3, 4, 6, 16) of every record to "DM ..." names; this script then checks that no name record still holds the source's
names (the copyright, trademark, designer, licence records, IDs 0, 7-14, stay as the licences ask) and refuses to write otherwise. nintendo_ext.ttf, whose letters are Noto glyphs, gets the copyright and
licence records (IDs 0, 13, 14) as well; its icons (circles, plates, arrows) are drawn by make_ext_font.py, not taken from any font."""
import argparse, hashlib, importlib.util, os, shutil, sys, tempfile
sys.dont_write_bytecode = True      # the engine's tree is read only: nothing may land in its __pycache__
try:
    from fontTools import subset
    from fontTools.misc import timeTools
    from fontTools.pens.cu2quPen import Cu2QuPen
    from fontTools.pens.ttGlyphPen import TTGlyphPen
    from fontTools.ttLib import TTFont, newTable
except ImportError:
    sys.exit('make-free-fonts: fontTools is needed (python3 -m venv V && V/bin/pip install fonttools, then V/bin/python3 make-free-fonts.py ...)')

FORBIDDEN = ('/System/Library',)    # Apple's fonts (Arial Unicode, Arial Rounded Bold) live there and must not be redistributed
OUT_FILES = ['standard.ttf', 'chinese_simplified.ttf', 'chinese_simplified_ext.ttf', 'chinese_traditional.ttf', 'korean.ttf', 'nintendo_ext.ttf']
PUA = range(0xE000, 0xF900)         # cp932's user-defined rows decode there: no font covers them, the engine's set asks for them anyway
# Name records that must not carry the source's names (family, unique, full, PostScript, typographic and WWS family, variations PostScript prefix)
NAME_IDS_RENAMED = (1, 3, 4, 6, 16, 21, 25)

OFL_TEXT = """This Font Software is licensed under the SIL Open Font License,
Version 1.1.

This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to
provide a free and open framework in which fonts may be shared and
improved in partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software
components as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to,
deleting, or substituting -- in part or in whole -- any of the
components of the Original Version, by changing formats or by porting
the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed,
modify, redistribute, and sell modified and unmodified copies of the
Font Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the
corresponding Copyright Holder. This restriction only applies to the
primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created using
the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
"""

APACHE_NOTICE = """Licensed under the Apache License, Version 2.0 (the "License"); you may not use these files except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the License for the specific language governing permissions and limitations under the License.
"""

def die(msg): sys.exit('make-free-fonts: ' + msg)

def check_path(label, p):
    # Refuse a path under /System/Library, as given and once symlinks are resolved
    for q in (os.path.abspath(os.path.expanduser(p)), os.path.realpath(os.path.expanduser(p))):
        for bad in FORBIDDEN:
            if q == bad or q.startswith(bad + os.sep):
                die('%s %r is under %s: Apple\'s system fonts must not be redistributed, use a freely licensed font' % (label, p, bad))

def load_module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec); sys.modules[name] = mod; spec.loader.exec_module(mod)
    return mod

def name_strings(font, ids):
    return [r.toUnicode() for r in font['name'].names if r.nameID in ids]

def first_name(font, nid):
    for want in ((3, 0x409), (1, 0), None):
        for r in font['name'].names:
            if r.nameID == nid and (want is None or (r.platformID, r.langID) == want): return r.toUnicode()
    return ''

def licence_kind(font):
    t = ' '.join(name_strings(font, (13, 14))).lower()
    if 'open font license' in t or 'scripts.sil.org/ofl' in t or 'openfontlicense' in t: return 'ofl'
    if 'apache license' in t or 'apache.org/licenses' in t: return 'apache'
    return None

def to_truetype(font, max_err):
    """CFF to glyf, in place (see the top of this file)."""
    order = font.getGlyphOrder(); gs = font.getGlyphSet(); quad = {}
    for n in order:
        pen = TTGlyphPen(gs)
        gs[n].draw(Cu2QuPen(pen, max_err, reverse_direction=True))
        quad[n] = pen.glyph(dropImpliedOnCurves=True)
    font['loca'] = newTable('loca'); font['glyf'] = g = newTable('glyf'); g.glyphOrder = order; g.glyphs = quad
    del font['CFF ']
    g.compile(font)                                   # recalculates every glyph's bounds
    hm = font['hmtx']
    for n in order:
        gl = g[n]; hm[n] = (hm[n][0], gl.xMin if gl.numberOfContours else 0)
    mp = font['maxp']; mp.tableVersion = 0x00010000; mp.maxZones = 1
    for k in ('maxTwilightPoints', 'maxStorage', 'maxFunctionDefs', 'maxInstructionDefs', 'maxStackElements', 'maxSizeOfInstructions', 'maxComponentElements', 'maxComponentDepth'):
        setattr(mp, k, 0)                             # unhinted; the other maxima are recalculated at save
    font['post'].formatType = 3.0
    font['head'].glyphDataFormat = 0
    font.sfntVersion = '\x00\x01\x00\x00'

def prepare_source(path, work, wanted, max_err, keep):
    """The font the engine's cut will read: a TrueType font as it is; an OTF/CFF font cut down to `wanted` and converted."""
    f = TTFont(path)
    if 'CFF ' not in f:
        print('source %s: TrueType outlines, used as it is' % os.path.basename(path))
        return path, False
    have = set(f.getBestCmap() or {}) & wanted
    o = subset.Options(hinting=False, layout_features=[], name_IDs=['*'], name_languages=['*'], notdef_outline=True, glyph_names=False)
    o.drop_tables = list(o.drop_tables) + ['BASE', 'VORG', 'vhea', 'vmtx', 'GSUB', 'GPOS', 'GDEF']     # the defaults already drop DSIG (invalid once the font is changed), JSTF, EBDT, EBLC, EBSC, PCLT, LTSH
    s = subset.Subsetter(o); s.populate(unicodes=have); s.subset(f)
    n0 = len(f.getGlyphOrder())
    to_truetype(f, max_err)
    out = os.path.join(work, 'source-truetype.ttf'); f.save(out)
    print('source %s: CFF to TrueType (max error %g units on a %d-unit em): %d characters, %d glyphs, %d bytes' % (os.path.basename(path), max_err, f['head'].unitsPerEm, len(have), n0, os.path.getsize(out)))
    if keep: shutil.copyfile(out, keep)
    return out, True

def add_licence_names(path, src):
    # The letters of nintendo_ext.ttf are glyphs of the source font: it carries the copyright and the licence like the cuts do (OFL 1.1, condition 2)
    f = TTFont(path, recalcTimestamp=False)
    for nid in (0, 13, 14):
        s = first_name(src, nid)
        if s:
            for plat, enc, lang in ((1, 0, 0), (3, 1, 0x409)): f['name'].setName(s, nid, plat, enc, lang)
    f.save(path)

def licence_text(src, kind, src_path, src_sha, max_err, converted):
    cr = first_name(src, 0) or '(the font has no copyright record)'
    fam = first_name(src, 1) or os.path.basename(src_path)
    L = ['Fonts in this folder: %s' % ', '.join(OUT_FILES), '',
         'They are derived from the font "%s" (%s, version: %s).' % (fam, os.path.basename(src_path), first_name(src, 5)),
         'Source font sha256: %s' % src_sha, 'Copyright: %s' % cr, 'Trademark notice of the font: %s' % first_name(src, 7),
         'Licence: %s' % ('SIL Open Font License, Version 1.1 (http://scripts.sil.org/OFL)' if kind == 'ofl' else 'Apache License, Version 2.0 (http://www.apache.org/licenses/LICENSE-2.0)'), '',
         'Changes%s: each file keeps only the glyphs of the characters of its type (standard, Simplified Chinese, its extension, Traditional Chinese, Korean),' % (
             " (a Modified Version in the OFL's words)" if kind == 'ofl' else ' (modified files, as the Apache License asks to state)'),
         'with the hinting program, the layout tables and the signature dropped%s; the family, full, unique and PostScript names are replaced by "DM ..." names, so no name of the original font is used.' % (
             ', and the outlines converted from CFF (cubic) to TrueType (quadratic) curves with an error of at most %g units on a 1000-unit em' % max_err if converted else ''),
         'nintendo_ext.ttf also holds controller-button icons drawn by a script (circles, plates, arrows: no font outlines) with the letters A B X Y L R ZL ZR SL SR taken from the source font.', '',
         '=' * 79, '']
    L.append(OFL_TEXT if kind == 'ofl' else APACHE_NOTICE)
    return '\n'.join(L)

def main():
    ap = argparse.ArgumentParser(description='Make the shared-font stand-ins from a freely licensed font (see the top of this file).')
    ap.add_argument('engine_tools_dir'); ap.add_argument('source_font'); ap.add_argument('out_dir')
    ap.add_argument('--max-err', type=float, default=1.0, help='CFF to TrueType: largest curve error in units of a 1000-unit em (default 1.0)')
    ap.add_argument('--license', dest='licence', choices=['ofl', 'apache'], help='the font\'s licence when its name table does not say it')
    ap.add_argument('--keep-converted', help='save the converted TrueType source font there')
    a = ap.parse_args()
    for label, p in (('engine tools folder', a.engine_tools_dir), ('source font', a.source_font), ('output folder', a.out_dir), ('--keep-converted file', a.keep_converted)):
        if p: check_path(label, p)
    tools = os.path.abspath(a.engine_tools_dir); out_dir = os.path.abspath(a.out_dir)
    if not os.path.isfile(os.path.join(tools, 'make_shared_fonts.py')) or not os.path.isfile(os.path.join(tools, 'make_ext_font.py')): die('make_shared_fonts.py / make_ext_font.py not found in %s' % tools)
    if not os.path.isfile(a.source_font): die('source font not found: %s' % a.source_font)
    for label, p in (('output folder', out_dir), ('--keep-converted file', a.keep_converted)):      # the engine's tree is read only
        if p and (os.path.realpath(p) == os.path.realpath(tools) or os.path.realpath(p).startswith(os.path.realpath(tools) + os.sep)): die('the %s is inside the engine tools folder, which is read only' % label)

    src = TTFont(a.source_font, lazy=True)
    kind = licence_kind(src) or a.licence
    if not kind: die('cannot tell the licence of %s from its name table (IDs 13/14): it must be a font under the SIL OFL or the Apache License; --license ofl|apache says it if you know' % a.source_font)
    with open(a.source_font, 'rb') as fh: src_sha = hashlib.sha256(fh.read()).hexdigest()
    leak = {s for s in name_strings(src, (1, 4, 6, 16)) if len(s) >= 3}      # the source's own names, which no output may carry
    # Deterministic output: fontTools stamps head.modified with the build time unless SOURCE_DATE_EPOCH is set
    os.environ['SOURCE_DATE_EPOCH'] = str(src['head'].modified + timeTools.epoch_diff)      # epoch_diff is negative (1904 to 1970)

    load_module(os.path.join(tools, 'make_ext_font.py'), 'make_ext_font')              # registered by name: make_shared_fonts.build() imports it and finds this one, from the engine's folder
    msf = load_module(os.path.join(tools, 'make_shared_fonts.py'), 'make_shared_fonts')
    sets = msf.charsets(); wanted = set().union(*sets.values())

    with tempfile.TemporaryDirectory(prefix='ibuilder-fonts-') as work:
        source, converted = prepare_source(a.source_font, work, wanted, a.max_err, a.keep_converted)
        msf.SOURCE = msf.LETTERS = source                  # the module's defaults name the free font too (build() below is given it explicitly all the same)
        stage = os.path.join(work, 'out'); msf.build(stage, source, source)             # the six files, exactly as the engine's script makes them
        srcfont = TTFont(a.source_font, lazy=True)
        add_licence_names(os.path.join(stage, msf.EXT), srcfont)
        # Checks before anything is written to out_dir
        cover = {t: sets[t] - set(PUA) for t in sets}; cm = set((TTFont(source, lazy=True).getBestCmap()) or {})
        rows = []
        for fn in OUT_FILES:
            p = os.path.join(stage, fn)
            if not os.path.isfile(p): die('the engine script did not make %s' % fn)
            f = TTFont(p, lazy=True)
            for nid in NAME_IDS_RENAMED:
                for s in name_strings(f, (nid,)):
                    if s and any(l in s or s in l for l in leak): die('%s: name record %d still holds a name of the source font (%r): the font would carry a Reserved Font Name' % (fn, nid, s))
            for t in ('fpgm', 'prep', 'cvt '):
                if t in f.reader.tables: die('%s still has a %s table: the cut must be unhinted' % (fn, t))
            if f['head'].unitsPerEm <= 0 or not any(t.platformID == 3 and t.platEncID == 1 for t in f['cmap'].tables): die('%s has no Windows Unicode cmap' % fn)
            rows.append((fn, os.path.getsize(p), f['maxp'].numGlyphs, len(f.getBestCmap()), f['maxp'].maxPoints, f['maxp'].maxContours, f.reader.tables['glyf'].length))
        os.makedirs(out_dir, exist_ok=True)
        for fn in OUT_FILES: shutil.copyfile(os.path.join(stage, fn), os.path.join(out_dir, fn))
        with open(os.path.join(out_dir, 'LICENSE-FONTS.txt'), 'w', encoding='utf-8', newline='\n') as fh:
            fh.write(licence_text(srcfont, kind, a.source_font, src_sha, a.max_err, converted))

    print('\n%-28s %9s %7s %7s %7s %7s %9s' % ('file', 'bytes', 'glyphs', 'chars', 'maxPts', 'maxCtr', 'glyf B'))
    for r in rows: print('%-28s %9d %7d %7d %7d %7d %9d' % r)
    for t, fn, _ in msf.TYPES:
        miss = sorted(cover[t] - cm)
        print('type %d %-27s needs %5d characters (without the private-use rows), the source lacks %5d (%.1f %%)%s' % (
            t, fn, len(cover[t]), len(miss), 100.0 * len(miss) / len(cover[t]), '   <== BADLY INCOMPLETE' if len(miss) > 0.05 * len(cover[t]) else ''))
    print('wrote %d fonts and LICENSE-FONTS.txt (%s) to %s' % (len(OUT_FILES), 'SIL OFL 1.1' if kind == 'ofl' else 'Apache-2.0', out_dir))
    for fn in OUT_FILES + ['LICENSE-FONTS.txt']:
        with open(os.path.join(out_dir, fn), 'rb') as fh: print('  sha256 %s  %s' % (hashlib.sha256(fh.read()).hexdigest(), fn))

if __name__ == '__main__':
    main()
