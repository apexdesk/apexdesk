' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
Sub Main()
    ActiveDocument.Range.ListFormat.ApplyBulletDefault
    Debug.Print (ActiveDocument.Paragraphs(1).Range.ListFormat.ListType <> wdListNoNumbering)
End Sub
