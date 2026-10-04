class TeXres < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/texres"
  version "0.5.2"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/texres/releases/download/v0.5.2/tex-suite-v0.5.2-macos-aarch64.tar.gz"
      sha256 "891aac0e141fe770451866700e52aefbe0e571f37545340c9f66dfa1e9a59c44"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/texres/releases/download/v0.5.2/tex-suite-v0.5.2-macos-x86_64.tar.gz"
      sha256 "0093cd3b3d016491ad315543db1e2ac973200f517e81e4f28a1966ff7c1672e5"
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
