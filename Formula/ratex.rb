class Ratex < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/ratex"
  version "0.5.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/ratex/releases/download/v0.5.0/tex-suite-v0.5.0-macos-aarch64.tar.gz"
      sha256 "ff83f2defb1b48fe771d1ea1e267c0772cd2228f8b4bb9544d724b2196008bc1"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/ratex/releases/download/v0.5.0/tex-suite-v0.5.0-macos-x86_64.tar.gz"
      sha256 "49756459ed7a6b02adc462e49d2537f9c44e758dc29566921139e05fbf0ab9dd"
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
