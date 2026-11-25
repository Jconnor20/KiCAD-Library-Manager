# Roadmap

What is planned, roughly in order. Nothing here is a promise or a date. Ideas and bug reports are welcome as GitHub issues. What does not work well today is in [LIMITS.md](LIMITS.md).

## Next

- [ ] **Linux and macOS builds.** KiCad runs on both. This needs KiCad discovery per OS (install and config paths), a keyring backend for API keys instead of Windows Credential Manager, the right editor names for Edit in KiCad (`eeschema`, `pcbnew`), and PCM package platforms beyond Windows. Verify through CI on each OS and, for Linux, through WSL2.

## Planned

**Checks and standards**

- [ ] **Source cross-check.** Flag a part field that disagrees with distributor data for its MPN. The code exists and is tested, but nothing calls it yet.
- [ ] **IC house symbol template.** ICs still get the automatic box symbol.
- [ ] **Naming rules in the standard profile**, so a shared profile carries category rules and name templates.
- [ ] **Symbol template preview** on the Part information page, using a real part from the library instead of sample values.
- [ ] **Create → Symbol:** write a new symbol from a pasted pin table into a library. Today it only shows a preview.
- [ ] **Per-fab findings in Project check "All fabs" mode**, not only which fabs pass.
- [ ] **Built-in fab and IPC profiles** (for example "JLC prototype", "IPC Class 3"), if a fab choice on a profile turns out not to be enough.
- [ ] **Value rebuilt for more than resistors, capacitors and inductors.**

**BOM and assembly**

- [ ] **Extended-part setup fee** in the BOM summary. No source gives it yet.
- [ ] **BOM import: look up only the selected lines,** with a per-line picker for symbol, footprint and 3D model. Make the Look up step clearer, since Generate needs it first.
- [ ] **Import alternatives found online directly,** from the Alternatives panel, with their CAD files.

**Import**

- [ ] **Move Import into Search or Library.** It may not need a page of its own.
- [ ] **Fusion 360 library import.** Only Eagle `.lbr` is read today.
- [ ] **Import ZIP handling of other file types** inside one ZIP (mixed symbol, footprint, model and datasheet files).
- [ ] **More part information** in Import from Search, before you import.
- [ ] **Faster footprint display** when importing from Search.
- [ ] **Distributor data for more fields:** country of origin, Nexar lead time, Mouser and Nexar fields checked against live replies.

**Working with KiCad**

- [ ] **Work fully with KiCad open.** Some `${KLM_LIB}` path rewrites still need KiCad closed.
- [ ] **Keep watching edits made in KiCad** across restarts. Today the watch lasts until KLM closes.

**Interface**

- [ ] **Sort arrows on every column.** Only the active column shows one.
- [ ] **Tidy the Library page action layout.** Edit, Preview, Attach, Copy to project and Open datasheet are still crowded.
- [ ] **Show the version in the app.**
- [ ] **Code signing** for `klm.exe`, so SmartScreen does not warn.

## Later

- [ ] **Team and shared libraries:** a shared folder or git repository, conflict detection, locking, and rules for merging metadata.
- [ ] **KiCad 9 output from a KiCad 10 install,** so files KLM writes still open in KiCad 9.
- [ ] **Native Altium and Eagle/Fusion export:** write `.SchLib`, `.PcbLib` and `.lbr` directly. The portable ZIP export works today for tools that read KiCad libraries.
- [ ] **Shipping cost and delivery-time estimates** in the BOM.
- [ ] **SnapMagic search inside the app,** if SnapMagic grants API access.

## Not planned

- **3D CAD editing.** It needs a CAD kernel. KLM checks and corrects a model's position and size; edit the model itself in FreeCAD or your CAD tool.
