' corpus-expect: ok
' corpus-may-change: yes
' corpus-note: by design a trusted location's macros, once allowed, change ordinary files anywhere (not hidden ones, programs or macro documents)
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
' corpus-trusted: yes
Sub Main()
    On Error Resume Next
    Kill "%HOME%\Documents\report.docx"
End Sub
