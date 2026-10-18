' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Check this
Sub Main()
    Range("A1").Value = 10
    Range("A1").AddComment "Check this"
    Debug.Print Range("A1").Comment.Text
End Sub
