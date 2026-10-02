class Ratex < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/ratex"
  version "0.4.7"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.7/tex-suite-v0.4.7-macos-aarch64.tar.gz"
      sha256 "4623359433c935382198d40113329d182caabcdbe47bb1cd4f8d95f0c0b09bf3"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/ratex/releases/download/v0.4.7/tex-suite-v0.4.7-macos-x86_64.tar.gz"
      sha256 "14e021b33e6f5c826ab9e20870fc6eac24382931c1b625ae6cf9d978ac4f1f93"
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
