Name:           ratex
Version:        0.4.0
Release:        1%{?dist}
Summary:        Ultra-fast, pure-Rust TeX engine and typesetting toolchain
License:        (MIT or Apache-2.0) and LPPL-1.3c and GPL-2.0-only and (GPL-2.0-or-later with Font-exception-2.0) and OFL-1.1 and GUST and Arphic and IPA and Wadalab
URL:            https://github.com/leoliu0/ratex
Source0:        %{name}-%{version}.tar.gz

Provides:       ratex

%description
Ratex is an ultra-fast, memory-safe, drop-in replacement for pdflatex and
latexmk with an embedded precompiled LaTeX format and near-instant startup.

%prep
%setup -q -n tex-suite-linux-x86_64

%install
rm -rf $RPM_BUILD_ROOT
mkdir -p $RPM_BUILD_ROOT%{_bindir}
mkdir -p $RPM_BUILD_ROOT%{_datadir}/tex-suite

./install.sh --prefix $RPM_BUILD_ROOT%{_prefix} --no-path --skip-verify --alias-latexmk
test -f $RPM_BUILD_ROOT%{_datadir}/tex-suite/texmf/doc/fonts/NOTICES-FONTS.txt || {
    echo "Error: required font notices missing after install" >&2
    exit 1
}
test -f $RPM_BUILD_ROOT%{_datadir}/tex-suite/texmf/doc/fonts/sources.tar.zst || {
    echo "Error: required font sources archive missing after install" >&2
    exit 1
}
test -f $RPM_BUILD_ROOT%{_datadir}/tex-suite/texmf/doc/fonts/packages.lock.json || {
    echo "Error: required font package lock missing after install" >&2
    exit 1
}

%files
%license LICENSE-MIT LICENSE-APACHE
%{_bindir}/ratex
%{_datadir}/tex-suite/texmf

%changelog
* Wed Sep 16 2026 Leo Liu <leoliu0@users.noreply.github.com> - 0.1.0-1
- Initial release
