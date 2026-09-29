class Ratex < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/ratex"
  version "0.4.5"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.5/tex-suite-v0.4.5-macos-aarch64.tar.gz"
      sha256 "f03c43d797498c0a1fafae6c79f4357603426a4cd807a66fd58fdb45bc245861"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.5/tex-suite-v0.4.5-macos-x86_64.tar.gz"
      sha256 "f090e1a7c57e3ee850d3cd16292ff4bef8d7ed738154db5b145f69fd2baefe87"
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
