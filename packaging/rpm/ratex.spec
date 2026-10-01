# Repackages the prebuilt Linux release archive.
%global debug_package %{nil}
%global __strip /bin/true

Name:           ratex
Version:        0.4.6
Release:        1%{?dist}
Summary:        Ultra-fast, pure-Rust TeX engine and typesetting toolchain
License:        (MIT or Apache-2.0) and LPPL-1.3c and GPL-2.0-only and (GPL-2.0-or-later with Font-exception-2.0) and OFL-1.1 and GUST and Arphic and IPA and Wadalab
URL:            https://github.com/leoliu0/ratex
Source0:        %{url}/releases/download/v%{version}/tex-suite-v%{version}-linux-x86_64.tar.gz
ExclusiveArch:  x86_64

%description
Ratex is an ultra-fast, memory-safe, drop-in replacement for pdflatex and
latexmk with an embedded precompiled LaTeX format and near-instant startup.

%prep
%setup -q -n tex-suite-linux-x86_64

%install
font_doc=share/tex-suite/texmf/doc/fonts
for required in NOTICES-FONTS.txt sources.tar.zst packages.lock.json; do
    test -f "$font_doc/$required" || {
        echo "Error: required font redistribution file missing: $font_doc/$required" >&2
        exit 1
    }
done
install -Dm755 bin/ratex %{buildroot}%{_bindir}/ratex
install -d %{buildroot}%{_datadir}/tex-suite
cp -a share/tex-suite/. %{buildroot}%{_datadir}/tex-suite/

%files
%license LICENSE-MIT LICENSE-APACHE
%{_bindir}/ratex
%{_datadir}/tex-suite

%changelog
* Fri Oct 02 2026 Leo Liu <leoliu0@users.noreply.github.com> - 0.4.6-1
- Install directly from the release archive instead of through install.sh

* Wed Sep 16 2026 Leo Liu <leoliu0@users.noreply.github.com> - 0.1.0-1
- Initial release
