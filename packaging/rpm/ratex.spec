Name:           ratex
Version:        0.1.0
Release:        1%{?dist}
Summary:        Ultra-fast, pure-Rust TeX engine and typesetting toolchain
License:        MIT or Apache-2.0
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

%files
%{_bindir}/ratex
%{_datadir}/tex-suite/texmf

%changelog
* Wed Sep 16 2026 Leo Liu <leoliu0@users.noreply.github.com> - 0.1.0-1
- Initial release
