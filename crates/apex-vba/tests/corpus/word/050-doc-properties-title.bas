' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Quarterly Report
' corpus-prints: Marta
Sub Main()
    ActiveDocument.BuiltInDocumentProperties("Title") = "Quarterly Report"
    ActiveDocument.BuiltInDocumentProperties("Author") = "Marta"
    Debug.Print ActiveDocument.BuiltInDocumentProperties("Title")
    Debug.Print ActiveDocument.BuiltInDocumentProperties("Author")
End Sub
