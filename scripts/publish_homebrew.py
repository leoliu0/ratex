#!/usr/bin/env python3
"""Publish a binary Homebrew formula using verified GitHub release digests."""
import json
from pathlib import Path
import re
import subprocess


def main():
    release = json.loads(subprocess.check_output([
        "gh", "release", "view", "--repo", "leoliu0/texres",
        "--json", "tagName,isDraft,isPrerelease,assets",
    ], text=True))
    tag = release["tagName"]
    if release["isDraft"] or release["isPrerelease"] or not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Expected a published stable release")
    assets = {asset["name"]: asset for asset in release["assets"]}
    blocks = []
    for arch, condition in [("aarch64", "Hardware::CPU.arm?"), ("x86_64", "Hardware::CPU.intel?")]:
        name = f"tex-suite-{tag}-macos-{arch}.tar.gz"
        asset = assets[name]
        digest = asset["digest"]
        if asset["state"] != "uploaded" or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
            raise ValueError(f"Missing verified digest for {name}")
        blocks.append(f'''    if {condition}
      url "https://github.com/leoliu0/texres/releases/download/{tag}/{name}"
      sha256 "{digest[7:]}"
    end''')
    formula = f'''class TeXres < Formula
  desc "Fast self-contained TeX engine written in Rust"
  homepage "https://github.com/leoliu0/texres"
  version "{tag[1:]}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
{chr(10).join(blocks)}
  end

  depends_on :macos

  def install
    bin.install "bin/texres"
    (share/"tex-suite").install Dir["share/tex-suite/*"]
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/texres --version")
    (testpath/"sample.tex").write <<~'LATEX'
      \\documentclass{{article}}
      \\usepackage{{hyperref}}
      \\begin{{document}}
      \\section{{Homebrew}}\\label{{home}}
      See \\ref{{home}}.
      \\end{{document}}
    LATEX
    system bin/"texres", testpath/"sample.tex"
    assert_path_exists testpath/"sample.pdf"
    assert_path_exists testpath/"sample.synctex.gz"
  end
end
'''
    path = Path("Formula/texres.rb")
    path.parent.mkdir(exist_ok=True)
    path.write_text(formula)
    print(f"Generated {path} for {tag}")


if __name__ == "__main__":
    main()
