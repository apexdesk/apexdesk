' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 0
Sub Main()
    Range("A1").AddComment "one"
    Range("A2").AddComment "two"
    Range("A1:A2").ClearComments
    Debug.Print ActiveSheet.Comments.Count
End Sub
