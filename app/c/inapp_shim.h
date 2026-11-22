/* The one function the iPhone app needs from the signing engine.
 *
 * The engine is zsign (https://github.com/zhlynn/zsign, MIT). tools/zsign_shim.cpp is the only adapter; nothing else
 * in iPorterNX touches zsign.
 *
 * Three details worth recording, all learned from zsign's source rather than from its README:
 *
 *  - zsign takes the team ID and the entitlements from the *provisioning profile*, not from the certificate.
 *    Signing with a certificate alone fails with "Can't find TeamId!", so the app's own embedded.mobileprovision is
 *    an argument here. The same profile is where entitlements come from, and on the iPhone a framework outside the
 *    bundle loads only when its signature carries them (probe 5C against 5A, docs/SIGNING.md); entitlementsPath is
 *    how a caller chooses, rather than leaving it to zsign.
 *  - ZBundle::SignFolder only signs a .app or an .appex; handed a .framework it stops at "Can't find app folder!".
 *    A framework is signed at the Mach-O level instead, the way zsign's own SignNode does it for a bundle nested in
 *    an app: one ZMachO, one identifier, the Info.plist hashed into the signature.
 *  - The resource seal is a file the signer must write (<framework>/_CodeSignature/CodeResources) and hash into the
 *    signature. zsign produces one only for an app, so the caller writes it and hands over its path.
 */
#ifndef NXPOCKET_ZSIGN_SHIM_H
#define NXPOCKET_ZSIGN_SHIM_H

#ifdef __cplusplus
extern "C" {
#endif

/* Signs one framework binary in place.
 *
 * p12Path            the certificate and its key, as exported from SideStore. A private temporary file, removed by
 *                    the caller; it is never stored anywhere permanent. It must arrive with its issuer's
 *                    certificate: GenerateCMS refuses a leaf whose issuer is not a chain it can present.
 * passphrase         the password that opens it.
 * provisionPath      the app's own embedded.mobileprovision, from which zsign reads the team ID.
 * entitlementsPath   a plist whose dict replaces the profile's entitlements: an empty dict for a framework that
 *                    carries no rights of its own. NULL or "" writes the profile's own entitlements into the
 *                    framework instead (probe test 5C, DeviceSigner's .appProfile); that needs
 *                    patches/zsign/0001-dylib-entitlements.patch, since stock zsign empties a dylib's entitlements.
 * frameworkPath      the .framework folder. Its Info.plist is hashed into the signature when it has one, and the
 *                    resource seal is written into it.
 * executableName     the Mach-O inside it, from the framework's CFBundleExecutable. Passed in rather than guessed:
 *                    the name is not always the framework's, and reading a plist in C++ would mean handling the
 *                    binary form.
 * identifier         the CodeDirectory identifier to write: the app's own, so iOS accepts the image outside the
 *                    bundle.
 * error              a NUL-terminated reason when the return value is non-zero.
 *
 * Returns 0 on success, non-zero on failure.
 */
int nxpocket_zsign_sign(const char *p12Path, const char *passphrase,
                        const char *provisionPath, const char *entitlementsPath,
                        const char *frameworkPath, const char *executableName, const char *identifier,
                        char *error, int errorLength);

/* Signs one framework binary in place with the app's own certificate, handed in as bytes rather than as files: the
 * .p12 and its password never have to be materialised on disk. The device path reads iPorterNX's Keychain item
 * (kSecAttrAccessibleWhenUnlockedThisDeviceOnly) and the app's own embedded.mobileprovision — both into memory — and
 * passes them straight down here. It needs sign/patches/zsign/0002-in-memory-asset.patch (ZSignAsset::InitFromMemory),
 * which is Init with the three file reads replaced by buffers.
 *
 * p12/p12Length               the .p12 bytes (certificate + key), out of the Keychain.
 * passphrase                  the password that opens it.
 * provision/provisionLength   the app's embedded.mobileprovision bytes, in memory.
 * entitlements/…Length        a plist whose dict replaces the profile's entitlements; NULL/0 writes the profile's own
 *                             (probe 5C), which needs patches/zsign/0001-dylib-entitlements.patch.
 * frameworkPath, executableName, identifier, error   as nxpocket_zsign_sign.
 *
 * Returns 0 on success, non-zero on failure.
 */
int nxpocket_zsign_sign_memory(const unsigned char *p12, int p12Length, const char *passphrase,
                               const unsigned char *provision, int provisionLength,
                               const unsigned char *entitlements, int entitlementsLength,
                               const char *frameworkPath, const char *executableName, const char *identifier,
                               char *error, int errorLength);

/* The certificate inside a .p12, read the way the signer will read it when it signs: OpenSSL, with its legacy
 * provider loaded as zsign does. iOS's own SecPKCS12Import refused a SideStore export that zsign signs with
 * (errSecDecode, -26275): the export carries no MAC, and Security rejects that. The MAC is checked only when there is
 * one, so the app checks a certificate with the same rules as the parser that is going to use it.
 *
 * Writes the leaf certificate's DER into `der` (derCapacity bytes) and its length into *derLength.
 *
 * Returns 0 on success; NXPOCKET_P12_WRONG_PASSWORD when the password does not open it; NXPOCKET_P12_UNREADABLE
 * when it is not a .p12 OpenSSL can decode; NXPOCKET_P12_NO_CERTIFICATE when it holds no certificate or no key;
 * NXPOCKET_P12_TOO_LARGE when der is too small. `error` then holds a reason.
 */
#define NXPOCKET_P12_WRONG_PASSWORD 1
#define NXPOCKET_P12_UNREADABLE 2
#define NXPOCKET_P12_NO_CERTIFICATE 3
#define NXPOCKET_P12_TOO_LARGE 4
int nxpocket_p12_certificate(const unsigned char *p12, int p12Length, const char *passphrase,
                             unsigned char *der, int derCapacity, int *derLength,
                             char *error, int errorLength);

/* Signs one framework binary in place with an AD-HOC signature — the path a process with no Apple identity can
 * take, and the one this repository's simulator proof exercises in-process.
 *
 * The simulator has no certificate and no provisioning profile; zsign's ad-hoc mode skips both (ZSignAsset::Init
 * returns before it asks for a p12 when bAdhoc is set) and removes the CMS slot from the SuperBlob, so what lands
 * on disk is a CodeDirectory and nothing else — the same shape `codesign -s -` and pipeline/adhoc_sign.py write.
 * The identifier is still the caller's: an ad-hoc CodeDirectory carries it, and the app reads its own out of its
 * own signature so the framework is signed *as this app* even here, not as a hardcoded name.
 *
 * frameworkPath      the .framework folder. Its Info.plist is hashed into the signature's info slot, and the
 *                    resource seal is written into it, exactly as the certified path does.
 * executableName     the Mach-O inside it (CFBundleExecutable).
 * identifier         the CodeDirectory identifier to write (the running app's own).
 * error              a NUL-terminated reason when the return value is non-zero.
 *
 * Returns 0 on success, non-zero on failure.
 */
int nxpocket_sign_adhoc(const char *frameworkPath, const char *executableName, const char *identifier,
                        char *error, int errorLength);

#ifdef __cplusplus
}
#endif

#endif /* NXPOCKET_ZSIGN_SHIM_H */
