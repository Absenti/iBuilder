/*
 * spvc_cli.c: the engine's `spirv-cross` command line, in process.
 *
 *   int gameapp_spirv_cross(int argc, const char **argv, char **out_stdout, char **out_stderr);
 *
 * argv[0] is the program name, as in main(). The function parses exactly the subset of spirv-cross's command line the
 * NX2Apple engine's shader chain uses (dmsh_build.py, dmsh_atomic.py, gl_shaders.py, vk_shaders.py) and drives the
 * SPIRV-Cross C API with what upstream main.cpp (vulkan-sdk-1.4.363.0, f11ba9f) would have set for the same flags, so
 * its output is byte-identical to the engine's binary (gameapp/tests and the proof in docs/ compare them file for file).
 * Any other flag is not guessed at: it fails with exit code 2 and a message on stderr.
 *
 * Supported:   <spv>  --output <path>  --msl  --msl-version <n>  --msl-argument-buffers  --msl-texture-buffer-native
 *              --msl-pad-fragment-output  --msl-decoration-binding  --msl-texture-1d-as-2d
 *              --rename-entry-point <old> <new> <stage>  --fixup-clipspace  --reflect [json]  --version <glsl n>
 *              --msl-argument-buffer-tier <n>  --flip-vert-y  --msl-discrete-descriptor-set <set>  --msl-dynamic-buffer <set> <binding>
 *              (the last four: vk_shaders.py and gl_shaders.py, ported in crates/ibuilder-glvk; at most MAX_SETS of each list)
 *              --revision  and a bare --version (which, as in the CLI, is a usage error: the help text, exit 1).
 * Without --msl and --reflect the target is plain GLSL, as upstream's default: combined image samplers built, the
 * dummy sampler, `--version 450` setting the #version.
 *
 * Results: the exit code is the CLI's, except that an output that could not be built in full (no memory) gives exit code 1 with
 * the outputs NULL (stderr: "out of memory" when that much can be allocated); a truncated text is never returned as a success. *out_stdout gets what the CLI prints on stdout (the text, when there is no
 * --output), *out_stderr what it prints on stderr; both are NUL-terminated malloc'd strings (empty, never NULL, when
 * there is nothing) the caller frees; either pointer argument may be NULL. --output files are written exactly as the CLI
 * does (fopen "w", the text, no extra newline); a file that cannot be opened or written in full is exit code 1 (upstream: 0, review r5 P2). Each call has its own spvc_context and no static state, so concurrent
 * calls are safe. Input "-" (stdin) is refused: the app has no stdin.
 *
 * Build: -I<out-spvc>/include, link libspvc-ios.a (or -sim) and -lc++ (tools/build_spvc_ios.sh).
 */
#include <errno.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "spirv_cross_c.h"

/* `failed` is sticky: once an append could not be made (no memory, or a size that does not fit), the text in the buffer is
 * incomplete and the caller must not present it as a result (gameapp_spirv_cross turns it into a non-zero exit code). */
typedef struct { char *p; size_t n, cap; int failed; } sbuf;

static void sb_add(sbuf *b, const char *s, size_t n)
{
	if (b->failed) return;
	if (n > SIZE_MAX - b->n - 1) { b->failed = 1; return; }
	if (b->n + n + 1 > b->cap) {
		size_t need = b->n + n + 1;
		size_t c = b->cap ? b->cap : 256;
		while (c < need) {
			if (c > SIZE_MAX / 2) { b->failed = 1; return; }   /* doubling would wrap: it would never reach `need` */
			c *= 2;
		}
		char *q = (char *)realloc(b->p, c);
		if (!q) { b->failed = 1; return; }
		b->p = q; b->cap = c;
	}
	memcpy(b->p + b->n, s, n);
	b->n += n;
	b->p[b->n] = 0;
}

static void sb_str(sbuf *b, const char *s) { sb_add(b, s, strlen(s)); }

static void sb_fmt(sbuf *b, const char *fmt, ...)
{
	char tmp[1024];
	va_list ap;
	va_start(ap, fmt);
	int n = vsnprintf(tmp, sizeof tmp, fmt, ap);
	va_end(ap);
	if (n < 0) { b->failed = 1; return; }
	if ((size_t)n < sizeof tmp) { sb_add(b, tmp, (size_t)n); return; }
	char *big = (char *)malloc((size_t)n + 1);
	if (!big) { b->failed = 1; return; }
	va_start(ap, fmt);
	vsnprintf(big, (size_t)n + 1, fmt, ap);
	va_end(ap);
	sb_add(b, big, (size_t)n);
	free(big);
}

/* what `spirv-cross --version` (a usage error) prints on stderr: print_version() and print_help() of this pin */
static const char k_help[] =
	"Git commit: vulkan-sdk-1.4.363.0 Timestamp: 2026-09-16T11:27:19\n"
	"Usage: spirv-cross [SPIR-V file] [options]\n"
	"\n"
	"Basic:\n"
	"\t[SPIR-V file] (- is stdin)\n"
	"\t[--output <output path>]: If not provided, prints output to stdout.\n"
	"\t[--help]:\n"
	"\t\tPrints this summary help message.\n"
	"\t[--help-all]:\n"
	"\t\tPrints all available help options.\n"
	"\n"
	"Select backend:\n"
	"\tBy default, OpenGL-style GLSL is the target, with #version and GLSL/ESSL information inherited from the SPIR-V module if present.\n"
	"\t[--vulkan-semantics] or [-V]:\n"
	"\t\tEmit Vulkan GLSL instead of plain GLSL. Makes use of Vulkan-only features to match SPIR-V.\n"
	"\t[--msl]:\n"
	"\t\tEmit Metal Shading Language (MSL).\n"
	"\t[--hlsl]:\n"
	"\t\tEmit HLSL.\n"
	"\t[--reflect]:\n"
	"\t\tEmit JSON reflection.\n"
	"\t[--cpp]:\n"
	"\t\tDEPRECATED. Emits C++ code.\n"
	"\n"
	"Common options:\n"
	"\t[--entry name]:\n"
	"\t\tUse a specific entry point. By default, the first entry point in the module is used.\n"
	"\t[--stage <stage (vert, frag, geom, tesc, tese, comp)>]:\n"
	"\t\tForces use of a certain shader stage.\n"
	"\t\tCan disambiguate the entry point if more than one entry point exists with same name, but different stage.\n"
	"\t[--emit-line-directives]:\n"
	"\t\tIf SPIR-V has OpLine directives, aim to emit those accurately in output code as well.\n"
	"\t[--rename-entry-point <old> <new> <stage>]:\n"
	"\t\tRenames an entry point from what is declared in SPIR-V to code output.\n"
	"\t\tMostly relevant for HLSL or MSL.\n"
	"\t[--rename-interface-variable <in|out> <location> <new_variable_name>]:\n"
	"\t\tRename an interface variable based on location decoration.\n"
	"\t[--force-zero-initialized-variables]:\n"
	"\t\tForces temporary variables to be initialized to zero.\n"
	"\t\tCan be useful in environments where compilers do not allow potentially uninitialized variables.\n"
	"\t\tThis usually comes up with Phi temporaries.\n"
	"\t[--fixup-clipspace]:\n"
	"\t\tFixup Z clip-space at the end of a vertex shader. The behavior is backend-dependent.\n"
	"\t\tGLSL: Rewrites [0, w] Z range (D3D/Metal/Vulkan) to GL-style [-w, w].\n"
	"\t\tHLSL/MSL: Rewrites [-w, w] Z range (GL) to D3D/Metal/Vulkan-style [0, w].\n"
	"\t[--flip-vert-y]:\n"
	"\t\tInverts gl_Position.y (or equivalent) at the end of a vertex shader. This is equivalent to using negative viewport height.\n"
	"\t[--mask-stage-output-location <location> <component>]:\n"
	"\t\tIf a stage output variable with matching location and component is active, optimize away the variable if applicable.\n"
	"\t[--mask-stage-output-builtin <Position|PointSize|ClipDistance|CullDistance>]:\n"
	"\t\tIf a stage output variable with matching builtin is active, optimize away the variable if it can affect cross-stage linking correctness.\n"
	"\t[--relax-nan-checks]:\n"
	"\t\tRelax NaN checks for N{Clamp,Min,Max} and ordered vs. unordered compare instructions.\n"
	"\n"
	"Help Categories:\n"
	"\t[--help-glsl]\n"
	"\t[--help-msl]\n"
	"\t[--help-hlsl]\n"
	"\t[--help-obscure]\n";

static const char k_revision[] = "Git commit: vulkan-sdk-1.4.363.0 Timestamp: 2026-09-16T11:27:19\n";

/* the CLI's stage names -> execution model (stage_to_execution_model); -1 when invalid (upstream throws "Invalid stage.") */
static int stage_model(const char *s)
{
	static const struct { const char *n; int m; } t[] = {
		{ "vert", SpvExecutionModelVertex }, { "frag", SpvExecutionModelFragment },
		{ "comp", SpvExecutionModelGLCompute }, { "tesc", SpvExecutionModelTessellationControl },
		{ "tese", SpvExecutionModelTessellationEvaluation }, { "geom", SpvExecutionModelGeometry },
		{ "rgen", SpvExecutionModelRayGenerationKHR }, { "rint", SpvExecutionModelIntersectionKHR },
		{ "rahit", SpvExecutionModelAnyHitKHR }, { "rchit", SpvExecutionModelClosestHitKHR },
		{ "rmiss", SpvExecutionModelMissKHR }, { "rcall", SpvExecutionModelCallableKHR },
		{ "mesh", SpvExecutionModelMeshEXT }, { "task", SpvExecutionModelTaskEXT },
	};
	for (size_t i = 0; i < sizeof t / sizeof t[0]; i++)
		if (!strcmp(s, t[i].n)) return t[i].m;
	return -1;
}

/* CLIParser::next_uint: stoul, then the 32-bit range check; any failure is the usage error */
static int parse_uint(const char *s, uint32_t *out)
{
	char *end;
	errno = 0;
	unsigned long long v = strtoull(s, &end, 10);
	if (end == s || errno || v > 0xffffffffull) return 0;
	*out = (uint32_t)v;
	return 1;
}

static char *dup_or_empty(const sbuf *b)
{
	char *r = (char *)malloc(b->n + 1);
	if (!r) return NULL;
	if (b->n) memcpy(r, b->p, b->n);
	r[b->n] = 0;
	return r;
}

enum { MAX_SETS = 64 };

typedef struct {
	const char *input, *output, *reflect;
	int msl, fixup, set_version, set_msl_version;
	int msl_argbuf, msl_texbuf, msl_pad, msl_decbind, msl_1d2d;
	uint32_t version, msl_version;
	const char *ren_old, *ren_new;
	int ren_model, have_ren;
	/* --msl-argument-buffer-tier (0 = Tier 1, upstream's default), --flip-vert-y, --msl-discrete-descriptor-set, --msl-dynamic-buffer (in command-line order: the
	 * dynamic buffers take the indexes 0, 1, ... as main.cpp's `i++`) */
	uint32_t msl_abtier;
	int yflip;
	uint32_t discrete[MAX_SETS]; size_t ndiscrete;
	uint32_t dyn_set[MAX_SETS], dyn_binding[MAX_SETS]; size_t ndyn;
} args_t;

/* --output: the text, exactly as the CLI writes it (fopen "w", the text, no extra newline). Returns the exit code: 0 when the whole text reached the file, 1 otherwise (review r5 P2: upstream
 * prints "Failed to write file" and exits 0 when the open fails, and checks no write; a full disk then left a truncated shader with a success code. The same rule as the buffers above: a
 * text that could not be delivered in full is a failure, not a shorter success). Same message as upstream, plus the reason. */
static int write_output(const char *path, const char *text, sbuf *se)
{
	FILE *fp = fopen(path, "w");
	if (!fp) { sb_fmt(se, "Failed to write file: %s (%s)\n", path, strerror(errno)); return 1; }
	size_t n = strlen(text);
	int bad = n > 0 && fwrite(text, 1, n, fp) != n;
	int err = bad ? errno : 0;
	if (fflush(fp) != 0 && !bad) { bad = 1; err = errno; }
	if (ferror(fp) && !bad) { bad = 1; err = EIO; }
	if (fclose(fp) != 0 && !bad) { bad = 1; err = errno; }
	if (bad) { sb_fmt(se, "Failed to write file: %s (%s)\n", path, strerror(err ? err : EIO)); return 1; }
	return 0;
}

static void opt_u(spvc_compiler_options o, spvc_compiler_option k, unsigned v) { spvc_compiler_options_set_uint(o, k, v); }

int gameapp_spirv_cross(int argc, const char **argv, char **out_stdout, char **out_stderr)
{
	sbuf so = { 0 }, se = { 0 };
	int rc = 0;
	args_t a;
	memset(&a, 0, sizeof a);
	spvc_context ctx = NULL;
	uint32_t *words = NULL;

	/* ---- the command line (CLIParser::parse, subset) ---- */
	for (int i = 1; i < argc; i++) {
		const char *f = argv[i];
		if (!f) { sb_str(&se, "spirv_cross: NULL argument\n"); rc = 2; goto done; }
		if (!strcmp(f, "--revision")) { sb_str(&se, k_revision); rc = 0; goto done; }
		else if (!strcmp(f, "--msl")) a.msl = 1;
		else if (!strcmp(f, "--msl-argument-buffers")) a.msl_argbuf = 1;
		else if (!strcmp(f, "--msl-texture-buffer-native")) a.msl_texbuf = 1;
		else if (!strcmp(f, "--msl-pad-fragment-output")) a.msl_pad = 1;
		else if (!strcmp(f, "--msl-decoration-binding")) a.msl_decbind = 1;
		else if (!strcmp(f, "--msl-texture-1d-as-2d")) a.msl_1d2d = 1;
		else if (!strcmp(f, "--fixup-clipspace")) a.fixup = 1;
		else if (!strcmp(f, "--flip-vert-y")) a.yflip = 1;
		else if (!strcmp(f, "--msl-argument-buffer-tier")) {
			if (i + 1 >= argc || !parse_uint(argv[++i], &a.msl_abtier)) goto usage;
		}
		else if (!strcmp(f, "--msl-discrete-descriptor-set")) {
			if (i + 1 >= argc || a.ndiscrete >= MAX_SETS || !parse_uint(argv[++i], &a.discrete[a.ndiscrete])) goto usage;
			a.ndiscrete++;
		}
		else if (!strcmp(f, "--msl-dynamic-buffer")) {
			if (i + 2 >= argc || a.ndyn >= MAX_SETS || !parse_uint(argv[i + 1], &a.dyn_set[a.ndyn]) || !parse_uint(argv[i + 2], &a.dyn_binding[a.ndyn])) goto usage;
			i += 2; a.ndyn++;
		}
		else if (!strcmp(f, "--output")) {
			if (i + 1 >= argc) goto usage;
			a.output = argv[++i];
		}
		else if (!strcmp(f, "--msl-version")) {
			if (i + 1 >= argc || !parse_uint(argv[++i], &a.msl_version)) goto usage;
			a.set_msl_version = 1;
		}
		else if (!strcmp(f, "--version")) {
			if (i + 1 >= argc || !parse_uint(argv[++i], &a.version)) goto usage;
			a.set_version = 1;
		}
		else if (!strcmp(f, "--reflect")) {
			/* the next word is the format unless it starts with "--" (upstream's default: json) */
			if (i + 1 < argc && strncmp(argv[i + 1], "--", 2) != 0) a.reflect = argv[++i];
			else a.reflect = "json";
		}
		else if (!strcmp(f, "--rename-entry-point")) {
			if (i + 3 >= argc) goto usage;
			a.ren_old = argv[i + 1]; a.ren_new = argv[i + 2];
			a.ren_model = stage_model(argv[i + 3]);
			i += 3;
			if (a.ren_model < 0) goto usage;
			a.have_ren = 1;
		}
		else if (f[0] == '-') {
			sb_fmt(&se, "spirv_cross: unsupported option \"%s\" (the in-process spirv-cross handles only the engine's command lines)\n", f);
			rc = 2; goto done;
		}
		else a.input = f;
	}
	goto parsed;
usage:
	sb_str(&se, k_help);
	rc = 1; goto done;
parsed:
	if (!a.input) {
		sb_str(&se, "Didn't specify input file.\n");
		sb_str(&se, k_help);
		rc = 1; goto done;
	}

	/* ---- read_spirv_file ---- */
	size_t nwords = 0;
	{
		FILE *fp = fopen(a.input, "rb");
		if (!fp) { sb_fmt(&se, "Failed to open SPIR-V file: %s\n", a.input); rc = 1; goto done; }
		fseek(fp, 0, SEEK_END);
		long len = ftell(fp) / (long)sizeof(uint32_t);
		rewind(fp);
		nwords = len > 0 ? (size_t)len : 0;
		words = (uint32_t *)malloc((nwords ? nwords : 1) * sizeof(uint32_t));
		if (!words) { sb_str(&se, "out of memory\n"); rc = 1; fclose(fp); goto done; }
		if (fread(words, sizeof(uint32_t), nwords, fp) != nwords) nwords = 0;
		fclose(fp);
		if (nwords == 0) { rc = 1; goto done; }   /* upstream returns EXIT_FAILURE without a message */
	}

	if (spvc_context_create(&ctx) != SPVC_SUCCESS) { sb_str(&se, "out of memory\n"); rc = 1; goto done; }

	spvc_parsed_ir ir;
	spvc_compiler comp = NULL;
	const char *text = NULL;
	spvc_result r;

	r = spvc_context_parse_spirv(ctx, words, nwords, &ir);
	if (r != SPVC_SUCCESS) goto thrown;

	if (a.reflect) {
		r = spvc_context_create_compiler(ctx, SPVC_BACKEND_JSON, ir, SPVC_CAPTURE_MODE_TAKE_OWNERSHIP, &comp);
		if (r != SPVC_SUCCESS) goto thrown;
		if (strcmp(a.reflect, "json") != 0) { sb_str(&se, "SPIRV-Cross threw an exception: Unsupported format\n"); rc = 1; goto done; }
		r = spvc_compiler_compile(comp, &text);
		if (r != SPVC_SUCCESS) goto thrown;
		goto emit;
	}

	r = spvc_context_create_compiler(ctx, a.msl ? SPVC_BACKEND_MSL : SPVC_BACKEND_GLSL, ir, SPVC_CAPTURE_MODE_TAKE_OWNERSHIP, &comp);
	if (r != SPVC_SUCCESS) goto thrown;

	{
		/* main.cpp's compile_iteration: the options it sets, with its defaults, for the flags above */
		spvc_compiler_options o;
		r = spvc_compiler_create_compiler_options(comp, &o);
		if (r != SPVC_SUCCESS) goto thrown;

		if (a.msl) {
			if (a.set_msl_version) opt_u(o, SPVC_COMPILER_OPTION_MSL_VERSION, a.msl_version);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_CAPTURE_OUTPUT_TO_BUFFER, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_SWIZZLE_TEXTURE_SAMPLES, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_INVARIANT_FP_MATH, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FRAMEBUFFER_FETCH_SUBPASS, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_PAD_FRAGMENT_OUTPUT_COMPONENTS, a.msl_pad);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_TESS_DOMAIN_ORIGIN_LOWER_LEFT, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ARGUMENT_BUFFERS, a.msl_argbuf);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ARGUMENT_BUFFERS_TIER, a.msl_abtier);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_TEXTURE_BUFFER_NATIVE, a.msl_texbuf);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_MULTIVIEW, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_MULTIVIEW_LAYERED_RENDERING, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_VIEW_INDEX_FROM_DEVICE_INDEX, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_DISPATCH_BASE, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_DECORATION_BINDING, a.msl_decbind);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FORCE_ACTIVE_ARGUMENT_BUFFER_RESOURCES, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FORCE_NATIVE_ARRAYS, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_FRAG_DEPTH_BUILTIN, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_FRAG_STENCIL_REF_BUILTIN, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_FRAG_OUTPUT_MASK, 0xffffffffu);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_CLIP_DISTANCE_USER_VARYING, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_RAW_BUFFER_TESE_INPUT, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_MULTI_PATCH_WORKGROUP, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_VERTEX_FOR_TESSELLATION, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ARRAYED_SUBPASS_INPUT, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_R32UI_LINEAR_TEXTURE_ALIGNMENT, 4);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_R32UI_ALIGNMENT_CONSTANT_ID, 65535);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_TEXTURE_1D_AS_2D, a.msl_1d2d);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_IOS_USE_SIMDGROUP_FUNCTIONS, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_EMULATE_SUBGROUPS, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FIXED_SUBGROUP_SIZE, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FORCE_SAMPLE_RATE_SHADING, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_MANUAL_HELPER_INVOCATION_UPDATES, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_CHECK_DISCARDED_FRAG_STORES, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_FORCE_FRAGMENT_WITH_SIDE_EFFECTS_EXECUTION, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_SAMPLE_DREF_LOD_ARRAY_AS_GRAD, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_IOS_SUPPORT_BASE_VERTEX_INSTANCE, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_REPLACE_RECURSIVE_INPUTS, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_READWRITE_TEXTURE_FENCES, 1);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_AGX_MANUAL_CUBE_GRAD_FIXUP, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_DISABLE_RASTERIZATION, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_AUTO_DISABLE_RASTERIZATION, 0);
			opt_u(o, SPVC_COMPILER_OPTION_MSL_ENABLE_POINT_SIZE_DEFAULT, 0);
		}
		/* the common block of options: set from args for every backend */
		if (a.set_version) opt_u(o, SPVC_COMPILER_OPTION_GLSL_VERSION, a.version);
		opt_u(o, SPVC_COMPILER_OPTION_FORCE_TEMPORARY, 0);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_SEPARATE_SHADER_OBJECTS, 0);
		opt_u(o, SPVC_COMPILER_OPTION_FLATTEN_MULTIDIMENSIONAL_ARRAYS, 0);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_ENABLE_420PACK_EXTENSION, 1);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_VULKAN_SEMANTICS, 0);
		opt_u(o, SPVC_COMPILER_OPTION_FIXUP_DEPTH_CONVENTION, a.fixup);
		opt_u(o, SPVC_COMPILER_OPTION_FLIP_VERTEX_Y, a.yflip);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_SUPPORT_NONZERO_BASE_INSTANCE, 1);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_EMIT_PUSH_CONSTANT_AS_UNIFORM_BUFFER, 0);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_EMIT_UNIFORM_BUFFER_AS_PLAIN_UNIFORMS, 0);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_FORCE_FLATTENED_IO_BLOCKS, 0);
		opt_u(o, SPVC_COMPILER_OPTION_GLSL_OVR_MULTIVIEW_VIEW_COUNT, 0);
		opt_u(o, SPVC_COMPILER_OPTION_EMIT_LINE_DIRECTIVES, 0);
		opt_u(o, SPVC_COMPILER_OPTION_ENABLE_STORAGE_IMAGE_QUALIFIER_DEDUCTION, 1);
		opt_u(o, SPVC_COMPILER_OPTION_FORCE_ZERO_INITIALIZED_VARIABLES, 0);
		opt_u(o, SPVC_COMPILER_OPTION_RELAX_NAN_CHECKS, 0);
		r = spvc_compiler_install_compiler_options(comp, o);
		if (r != SPVC_SUCCESS) goto thrown;
		if (a.msl) {
			/* main.cpp, after set_msl_options: the discrete sets, then the dynamic buffers numbered in their order */
			for (size_t k = 0; k < a.ndiscrete; k++) {
				r = spvc_compiler_msl_add_discrete_descriptor_set(comp, a.discrete[k]);
				if (r != SPVC_SUCCESS) goto thrown;
			}
			for (size_t k = 0; k < a.ndyn; k++) {
				r = spvc_compiler_msl_add_dynamic_buffer(comp, a.dyn_set[k], a.dyn_binding[k], (unsigned)k);
				if (r != SPVC_SUCCESS) goto thrown;
			}
		}
	}

	if (a.have_ren) {
		r = spvc_compiler_rename_entry_point(comp, a.ren_old, a.ren_new, (SpvExecutionModel)a.ren_model);
		if (r != SPVC_SUCCESS) goto thrown;
	}

	{
		spvc_resources res;
		if (!a.msl) {
			/* plain GLSL: the dummy sampler, then the combined image samplers named as the CLI names them */
			spvc_variable_id sampler = 0;
			r = spvc_compiler_build_dummy_sampler_for_combined_images(comp, &sampler);
			if (r != SPVC_SUCCESS) goto thrown;
			if (sampler != 0) {
				spvc_compiler_set_decoration(comp, sampler, SpvDecorationDescriptorSet, 0);
				spvc_compiler_set_decoration(comp, sampler, SpvDecorationBinding, 0);
			}
		}
		r = spvc_compiler_create_shader_resources(comp, &res);
		if (r != SPVC_SUCCESS) goto thrown;
		if (!a.msl) {
			r = spvc_compiler_build_combined_image_samplers(comp);
			if (r != SPVC_SUCCESS) goto thrown;
			const spvc_combined_image_sampler *cs = NULL;
			size_t ncs = 0;
			r = spvc_compiler_get_combined_image_samplers(comp, &cs, &ncs);
			if (r != SPVC_SUCCESS) goto thrown;
			for (size_t i = 0; i < ncs; i++) {
				const char *in = spvc_compiler_get_name(comp, cs[i].image_id);
				const char *sn = spvc_compiler_get_name(comp, cs[i].sampler_id);
				sbuf nm = { 0 };
				sb_str(&nm, "SPIRV_Cross_Combined");
				sb_str(&nm, in ? in : "");
				sb_str(&nm, sn ? sn : "");
				if (nm.failed || !nm.p) { free(nm.p); sb_str(&se, "spirv_cross: out of memory\n"); se.failed = 1; rc = 1; goto done; }
				spvc_compiler_set_name(comp, cs[i].combined_id, nm.p);
				free(nm.p);
			}
		}
	}

	r = spvc_compiler_compile(comp, &text);
	if (r != SPVC_SUCCESS) {
		/* upstream prints an empty result without complaint; the C API reports "Unsupported SPIR-V." for it */
		if (r == SPVC_ERROR_UNSUPPORTED_SPIRV && !strcmp(spvc_context_get_last_error_string(ctx), "Unsupported SPIR-V."))
			text = "";
		else goto thrown;
	}

emit:
	if (a.output) rc = write_output(a.output, text, &se);
	else { sb_str(&so, text); rc = 0; }
	goto done;

thrown:
	sb_fmt(&se, "SPIRV-Cross threw an exception: %s\n", spvc_context_get_last_error_string(ctx));
	rc = 1;

done:
	if (ctx) spvc_context_destroy(ctx);   /* frees `text` too: it was written out above */
	free(words);
	/* an output that could not be built in full is a failure, not a shorter success */
	char *dso = out_stdout ? dup_or_empty(&so) : NULL, *dse = out_stderr ? dup_or_empty(&se) : NULL;
	if (so.failed || se.failed || (out_stdout && !dso) || (out_stderr && !dse)) {
		if (rc == 0) rc = 1;
		free(dso); dso = NULL;
		if (out_stdout) *out_stdout = NULL;
		free(dse);
		dse = out_stderr ? strdup("spirv_cross: out of memory\n") : NULL;   /* NULL if even that fails: the caller reads NULL as empty */
		if (out_stderr) *out_stderr = dse;
	} else {
		if (out_stdout) *out_stdout = dso;
		if (out_stderr) *out_stderr = dse;
	}
	free(so.p); free(se.p);
	return rc;
}
