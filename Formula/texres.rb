class Texres < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/texres"
  version "0.7.6"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.6/tex-suite-v0.7.6-macos-aarch64.tar.gz"
      sha256 "7576c548ec997716feac1ba8410c1c832437b13c82f02272709f843367739d5a"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.6/tex-suite-v0.7.6-macos-x86_64.tar.gz"
      sha256 "6f39bbd06b93789e82e2cb89c04fff6e762b458e1614e44df4d2649e11732fca"
    end
  end

  depends_on :macos

  def install
    bin.install "bin/texres"
    (share/"tex-suite").install Dir["share/tex-suite/*"]
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/texres --version")
    (testpath/"sample.tex").write <<~'LATEX'
      \documentclass{article}
      \usepackage{hyperref}
      \begin{document}
      \section{Homebrew}\label{home}
      See \ref{home}.
      \end{document}
    LATEX
    system bin/"texres", testpath/"sample.tex"
    assert_path_exists testpath/"sample.pdf"
    assert_path_exists testpath/"sample.synctex.gz"
  end
end
