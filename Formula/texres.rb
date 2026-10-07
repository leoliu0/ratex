class Texres < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/texres"
  version "0.7.3"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.3/tex-suite-v0.7.3-macos-aarch64.tar.gz"
      sha256 "fbd00a089b1ca48a41ca284ab8598b6cb3e0f0c3e9c6ee8de08292147ba4da94"
    end
    if Hardware::CPU.intel?
      url "https://github.com/leoliu0/texres/releases/download/v0.7.3/tex-suite-v0.7.3-macos-x86_64.tar.gz"
      sha256 "4cc6baf605f73ac11229a5df29f147cf6a0b856168a0c796daea472556228fc1"
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
