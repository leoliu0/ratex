class Texres < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/texres"
  version "0.7.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.1/tex-suite-v0.7.1-macos-aarch64.tar.gz"
      sha256 "0ef3ff2162ab4a7f843ea69e3d4f2c95ef1a6f13e169fcb2cca9703fd0b73611"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.1/tex-suite-v0.7.1-macos-x86_64.tar.gz"
      sha256 "bdc128651cea0e5a3891ffa18b0c5c81d3ef3fc5a765c02893376da33c1bc1c4"
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
