class Ratex < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/ratex"
  version "0.4.6"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.6/tex-suite-v0.4.6-macos-aarch64.tar.gz"
      sha256 "bdfcb907aa7c737706cc968643d9ecb727e56157e4787cb0b2281567126a9b21"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.6/tex-suite-v0.4.6-macos-x86_64.tar.gz"
      sha256 "351e43531d2425063f1ab65111cc83fa86de2d95f40684445bbfa8d7bf33fc93"
    end
  end

  depends_on :macos

  def install
    bin.install "bin/ratex"
    (share/"tex-suite").install Dir["share/tex-suite/*"]
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ratex --version")
    (testpath/"sample.tex").write <<~'LATEX'
      \documentclass{article}
      \usepackage{hyperref}
      \begin{document}
      \section{Homebrew}\label{home}
      See \ref{home}.
      \end{document}
    LATEX
    system bin/"ratex", testpath/"sample.tex"
    assert_path_exists testpath/"sample.pdf"
    assert_path_exists testpath/"sample.synctex.gz"
  end
end
