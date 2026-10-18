' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
' corpus-prints: banana
' corpus-prints: 2
Sub Main()
    Dim col As New Collection
    col.Add "apple"
    col.Add "banana"
    col.Add "cherry"
    Debug.Print col.Count
    Debug.Print col(2)
    col.Remove 1
    Debug.Print col.Count
End Sub
