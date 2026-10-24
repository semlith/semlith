# The README's recording: semlith answering from a PDF and a Word file.
#
# Run from a folder holding `docs/` (seven public files from the documents
# benchmark, bench/scorecard/documents/manifest.jsonl: the Transformer and BERT
# papers, NIST CSF 2.0, two DfE Word reports, a DfE slide deck, a CDC workbook)
# after `semlith --store .semlith index docs`:
#
#   vhs <repo>/docs/images/demo.tape
#
# then move demo.gif into docs/images/.

Output demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1200
Set Height 460
Set Padding 20
Set Theme "GitHub Dark"
Set TypingSpeed 25ms

Hide
Type "export PS1='$ '" Enter
Type "clear" Enter
Show

Type "ls docs" Sleep 400ms Enter
Sleep 1.5s

Type `semlith --store .semlith search -k 1 "why divide the dot products by the square root of the key dimension"`
Sleep 400ms Enter
Sleep 4s

Type "clear" Enter
Type `semlith --store .semlith search -k 1 "link between speech and language needs and behaviour problems"`
Sleep 400ms Enter
Sleep 5s
