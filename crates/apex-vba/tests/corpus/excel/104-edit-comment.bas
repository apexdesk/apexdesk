' corpus-source: generated
' corpus-flagged: no
' corpus-prints: updated
' corpus-prints: 1
Sub Main()
    Range("B2").AddComment "first"
    Range("B2").Comment.Text "updated"
    Debug.Print Range("B2").Comment.Text
    Debug.Print ActiveSheet.Comments.Count
End Sub
