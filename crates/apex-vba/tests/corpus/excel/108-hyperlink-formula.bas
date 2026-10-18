' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Docs
Sub Main()
    Range("A1").Formula = "=HYPERLINK(""https://example.com/docs"",""Docs"")"
    Debug.Print Range("A1").Value
End Sub
